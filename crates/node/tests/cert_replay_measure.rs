//! Measurement for ADR-052: what replaying one epoch of certificates costs.
//!
//! Ignored by default. Run with
//! `CERT_N=4 CERT_BLOCKS=3600 cargo test -p custom-l1-node --test cert_replay_measure -- --ignored --nocapture`.
//!
//! It runs an in-process committee to `CERT_BLOCKS`, keeps every certificate
//! that crossed the wire, then times two things on a fresh observer:
//! verifying the `n - f` signatures of every certificate, and replaying the
//! certificates through the engine to re-derive the blocks.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use custom_l1_node::consensus::bft::auth::MlDsaAuthenticator;
use custom_l1_node::consensus::bft::{BROADCAST, BftDriver, BftSetup, Envelope, Step};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::keys::{self, SigningKey, VerifyingKey};
use custom_l1_node::genesis::{Allocation, BftGenesis, GenesisBond, GenesisConfig, StakingGenesis};
use custom_l1_node::state::db::StateDB;
use maya_dag_bft::{Authenticator, Certificate, Message, Params};
use tempfile::TempDir;

mod common;

const EPOCH_BLOCKS: u64 = 3600;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn operator() -> HybridSigningKey {
    signing_key_from_seed(&[0x42; 32]).unwrap()
}

fn validator_key(i: usize) -> Arc<SigningKey> {
    Arc::new(keys::signing_key_from_seed(&[u8::try_from(i).unwrap() + 1; 32]).unwrap())
}

fn genesis_config(n: usize) -> GenesisConfig {
    let validators = (0..n)
        .map(|i| hex::encode(validator_key(i).verifying_key().to_bytes()))
        .collect();
    let op = hex::encode(operator().address());
    GenesisConfig {
        chain_id: "cert-replay-measure".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: vec![Allocation {
            address: op.clone(),
            balance: primitive_types::U256::from(1_000_000),
        }],
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: Some(BftGenesis {
            validators,
            anchor_timeout_ms: 1_000,
            batch_size: 100,
            round_interval_ms: 0,
            fees: None,
            mining: None,
            staking: Some(StakingGenesis {
                epoch_blocks: EPOCH_BLOCKS,
                bonds: (0..n)
                    .map(|_| GenesisBond {
                        operator: op.clone(),
                        bond: primitive_types::U256::from(10_000),
                        commission_bps: 0,
                    })
                    .collect(),
                min_self_bond: primitive_types::U256::from(1_000),
                max_validators: 100,
                stake_weighted: false,
            }),
        }),
    }
}

fn open_chain(config: &GenesisConfig, dir: &std::path::Path) -> Chain {
    let state = Arc::new(StateDB::open(dir).unwrap());
    common::open_chain(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

struct Member {
    driver: BftDriver,
    chain: Chain,
}

/// Runs the committee and keeps each distinct certificate it sees.
struct Run {
    _root: TempDir,
    members: Vec<Member>,
    queue: VecDeque<(usize, Vec<u8>)>,
    certs: Vec<Certificate>,
    seen: HashSet<(u64, u16)>,
    now: u64,
}

impl Run {
    fn new(n: usize, config: &GenesisConfig) -> Self {
        let root = TempDir::new().unwrap();
        let committee: Arc<[VerifyingKey]> = config
            .bft
            .as_ref()
            .unwrap()
            .verifying_keys()
            .unwrap()
            .into();
        let params = Params {
            batch_size: 100,
            min_round_interval_ms: 200,
            ..Params::default()
        };
        let mut members = Vec::new();
        let mut opening = Vec::new();
        for i in 0..n {
            let setup = BftSetup {
                epoch: 0,
                committee: Arc::clone(&committee),
                signer: Some(validator_key(i).into()),
                params,
            };
            let dir = root.path().join(format!("state-{i}"));
            let mut chain = open_chain(config, &dir);
            let (driver, step) = BftDriver::open(
                &setup,
                &root.path().join(format!("bft-{i}")),
                &mut chain,
                0,
            )
            .unwrap();
            opening.push((i, step));
            members.push(Member { driver, chain });
        }
        let mut run = Self {
            _root: root,
            members,
            queue: VecDeque::new(),
            certs: Vec::new(),
            seen: HashSet::new(),
            now: 0,
        };
        for (i, step) in opening {
            run.absorb(i, step);
        }
        run
    }

    fn absorb(&mut self, from: usize, step: Step) {
        self.queue
            .extend(step.frames.into_iter().map(|f| (from, f)));
    }

    fn capture(&mut self, frame: &[u8]) {
        let Ok(envelope) = Envelope::decode(frame) else {
            return;
        };
        if let Message::Cert(c) = envelope.message
            && self.seen.insert((c.vertex.round, c.vertex.author))
        {
            self.certs.push(c);
        }
    }

    fn pump(&mut self, budget: usize) {
        for _ in 0..budget {
            let Some((from, frame)) = self.queue.pop_front() else {
                return;
            };
            self.capture(&frame);
            for i in 0..self.members.len() {
                if i == from {
                    continue;
                }
                let m = &mut self.members[i];
                let step = m.driver.on_frame(&mut m.chain, self.now, &frame).unwrap();
                self.absorb(i, step);
            }
        }
    }

    fn tick(&mut self) {
        for i in 0..self.members.len() {
            let m = &mut self.members[i];
            let step = m.driver.on_tick(&mut m.chain, self.now).unwrap();
            self.absorb(i, step);
        }
    }

    fn run_to(&mut self, height: u64) {
        let mut rounds = 0_u64;
        while self.members[0].chain.height() < height {
            self.now += 250;
            self.tick();
            self.pump(3_000);
            rounds += 1;
            assert!(rounds < 2_000_000, "the committee stalled");
        }
    }
}

#[test]
#[ignore = "measurement for ADR-052; run with --ignored"]
fn replaying_an_epoch_of_certificates_costs_this_much() {
    let n = env_usize("CERT_N", 4);
    let blocks = u64::try_from(env_usize("CERT_BLOCKS", 3600)).unwrap();
    let f = (n - 1) / 3;
    let quorum = n - f;
    let config = genesis_config(n);
    let committee: Arc<[VerifyingKey]> = config
        .bft
        .as_ref()
        .unwrap()
        .verifying_keys()
        .unwrap()
        .into();

    let t = Instant::now();
    let mut run = Run::new(n, &config);
    run.run_to(blocks);
    println!(
        "RUN n={n} blocks={} certs={} run_secs={:.1}",
        run.members[0].chain.height(),
        run.certs.len(),
        t.elapsed().as_secs_f64()
    );
    let mut certs = run.certs.clone();
    certs.sort_by_key(|c| (c.vertex.round, c.vertex.author));

    // 1. Verify every certificate's signatures, as a follower must.
    let auth = MlDsaAuthenticator::observer(Arc::clone(&committee));
    let t = Instant::now();
    let mut valid_sigs = 0_usize;
    for c in &certs {
        let digest = c.digest();
        assert!(c.votes.len() >= quorum, "insufficient votes in round {}", c.vertex.round);
        for (voter, sig) in c.votes.iter().zip(&c.signatures) {
            assert!(auth.verify(*voter, &digest, sig), "bad signature");
            valid_sigs += 1;
        }
    }
    println!(
        "VERIFY n={n} certs={} sigs={valid_sigs} verify_secs={:.2}",
        certs.len(),
        t.elapsed().as_secs_f64()
    );

    // 2. Replay them into a fresh observer and time the re-derivation.
    let fresh = TempDir::new().unwrap();
    let setup = BftSetup {
        epoch: 0,
        committee: Arc::clone(&committee),
        signer: None,
        params: Params {
            batch_size: 100,
            min_round_interval_ms: 200,
            ..Params::default()
        },
    };
    let mut chain = open_chain(&config, &fresh.path().join("state"));
    let (mut observer, _) = BftDriver::open(&setup, &fresh.path().join("bft"), &mut chain, 0).unwrap();
    let t = Instant::now();
    let mut now = 0_u64;
    for c in certs {
        let envelope = Envelope {
            epoch: 0,
            from: c.vertex.author,
            to: BROADCAST,
            message: Message::Cert(c),
        };
        observer.on_frame(&mut chain, now, &envelope.encode()).unwrap();
        now += 1;
    }
    observer.on_tick(&mut chain, now).unwrap();
    let replay_secs = t.elapsed().as_secs_f64();
    let want = run.members[0].chain.height();
    println!(
        "REPLAY n={n} height={} of {want} replay_secs={replay_secs:.2}",
        chain.height()
    );
    let mine = chain.active_chain().unwrap();
    let theirs = run.members[0].chain.active_chain().unwrap();
    let shared = usize::try_from(chain.height().min(want)).unwrap();
    assert_eq!(mine[..=shared], theirs[..=shared], "replayed blocks differ");
    println!("AGREE n={n} shared_blocks={shared}");
}
