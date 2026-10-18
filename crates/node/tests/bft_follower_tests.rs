//! Two validators restarted into attested-follower mode while the chain keeps
//! advancing, then the chain crosses an epoch boundary (ADR-038, ADR-043).
//!
//! The 2026-10-18 incident on maya-testnet-1: v2 and v3 restarted with
//! `--catch-up-from` at tip 64,321. Both imported nothing and followed. The
//! chain halted at the next epoch boundary, because followers switch epochs
//! only by importing the boundary block, and a checkpoint needs `n - f`
//! attesters that followers are not. This test pins that every member keeps
//! advancing its tip, builds or imports blocks, and agrees across the boundary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use custom_l1_node::consensus::bft::certsync::{self, CertSource, PAGE_BYTES};
use custom_l1_node::consensus::bft::store::{CertPage, read_certificate_page};
use custom_l1_node::consensus::bft::wire::{decode_certificate, encode_certificate};
use custom_l1_node::consensus::bft::{BftDriver, BftSetup, Step};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::keys::{self, SigningKey, VerifyingKey};
use custom_l1_node::genesis::{Allocation, BftGenesis, GenesisBond, GenesisConfig, StakingGenesis};
use custom_l1_node::state::db::StateDB;
use maya_dag_bft::Params;
use tempfile::TempDir;

mod common;

const VALIDATORS: usize = 4;
const EPOCH_BLOCKS: u64 = 6;

fn operator() -> HybridSigningKey {
    signing_key_from_seed(&[0x42; 32]).unwrap()
}

fn validator_key(i: usize) -> Arc<SigningKey> {
    Arc::new(keys::signing_key_from_seed(&[u8::try_from(i).unwrap() + 1; 32]).unwrap())
}

fn genesis_config() -> GenesisConfig {
    let validators = (0..VALIDATORS)
        .map(|i| hex::encode(validator_key(i).verifying_key().to_bytes()))
        .collect();
    let op = hex::encode(operator().address());
    GenesisConfig {
        chain_id: "bft-follower-test".to_string(),
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
                bonds: (0..VALIDATORS)
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

struct Member {
    driver: Option<BftDriver>,
    chain: Chain,
    state_dir: PathBuf,
    bft_dir: PathBuf,
}

struct Mesh {
    root: TempDir,
    // Kept so the directory outlives every chain in `members`.
    config: GenesisConfig,
    setups: Vec<BftSetup>,
    members: Vec<Member>,
    queue: VecDeque<(usize, Vec<u8>)>,
    now: u64,
    equivocations: usize,
}

fn open_chain(config: &GenesisConfig, state_dir: &PathBuf) -> Chain {
    let state = Arc::new(StateDB::open(state_dir).unwrap());
    common::open_chain(
        state,
        config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

impl Mesh {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let config = genesis_config();
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
        let mut setups = Vec::new();
        let mut members = Vec::new();
        let mut opening = Vec::new();
        for i in 0..VALIDATORS {
            let state_dir = root.path().join(format!("state-{i}"));
            let bft_dir = root.path().join(format!("bft-{i}"));
            let setup = BftSetup {
                epoch: 0,
                committee: Arc::clone(&committee),
                signer: Some(validator_key(i).into()),
                params,
            };
            let state = Arc::new(StateDB::open(&state_dir).unwrap());
            config.seed_state(&state).unwrap();
            let mut chain = common::open_chain(
                state,
                config.genesis_block().unwrap(),
                ChainConfig::without_pow_verification(),
            )
            .unwrap();
            let (driver, step) = BftDriver::open(&setup, &bft_dir, &mut chain, 0).unwrap();
            opening.push((i, step));
            members.push(Member {
                driver: Some(driver),
                chain,
                state_dir,
                bft_dir,
            });
            setups.push(setup);
        }
        let mut mesh = Self {
            root,
            config,
            setups,
            members,
            queue: VecDeque::new(),
            now: 0,
            equivocations: 0,
        };
        for (i, step) in opening {
            mesh.absorb(i, step);
        }
        mesh
    }

    fn absorb(&mut self, from: usize, step: Step) {
        self.equivocations += step.equivocations.len();
        self.queue
            .extend(step.frames.into_iter().map(|f| (from, f)));
    }

    fn tick(&mut self) {
        for i in 0..self.members.len() {
            let m = &mut self.members[i];
            let Some(driver) = m.driver.as_mut() else {
                continue;
            };
            let step = driver.on_tick(&mut m.chain, self.now).unwrap();
            self.absorb(i, step);
        }
    }

    fn pump(&mut self, budget: usize) {
        for _ in 0..budget {
            let Some((from, frame)) = self.queue.pop_front() else {
                return;
            };
            for i in 0..self.members.len() {
                if i == from {
                    continue;
                }
                let m = &mut self.members[i];
                let Some(driver) = m.driver.as_mut() else {
                    continue;
                };
                let step = driver.on_frame(&mut m.chain, self.now, &frame).unwrap();
                self.absorb(i, step);
            }
        }
    }

    /// Reopens member `i`'s chain from its disk, releasing the old handle
    /// first, as a restart would.
    fn reopen_chain(&mut self, i: usize) {
        let scratch = self.root.path().join(format!("scratch-{i}"));
        self.members[i].chain = open_chain(&self.config, &scratch);
        let state_dir = self.members[i].state_dir.clone();
        self.members[i].chain = open_chain(&self.config, &state_dir);
    }

    /// Member `from`'s certificate log, as a catch-up source.
    fn peer_log(&self, from: usize) -> PeerLog {
        PeerLog {
            dir: self.members[from].bft_dir.clone(),
            epoch: self.members[from].driver.as_ref().unwrap().epoch(),
        }
    }

    /// Restarts member `i` the way an operator does with `--catch-up-from`:
    /// certificate replay from member `from` (ADR-052), then the engine. No
    /// checkpoint and no follower mode.
    fn restart_from_certificates(&mut self, i: usize, from: usize) {
        self.reopen_chain(i);
        let source = self.peer_log(from);
        let setup = &self.setups[i];
        let m = &mut self.members[i];
        let synced = certsync::sync(&mut m.chain, &source, setup, &m.bft_dir, self.now).unwrap();
        m.driver = Some(synced.driver);
        self.absorb(i, synced.step);
    }

    /// Runs `ticks` rounds of the clock without the usual stop condition.
    fn run_ticks(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.now += 250;
            self.tick();
            self.pump(3_000);
        }
    }

    /// Runs until `done`, letting followers catch up from member 0 each step.
    fn run_until(&mut self, done: impl Fn(&Self) -> bool) {
        for _ in 0..800 {
            if done(self) {
                return;
            }
            self.now += 250;
            self.tick();
            self.pump(3_000);
        }
    }

    fn hold_out(&mut self, i: usize) {
        self.members[i].driver = None;
    }

    fn height(&self, i: usize) -> u64 {
        self.members[i].chain.height()
    }

    fn epoch(&self, i: usize) -> Option<u64> {
        self.members[i].driver.as_ref().map(BftDriver::epoch)
    }

    /// Every live member's block at the lowest common height has one id.
    fn agree(&self) {
        let common = self.members.iter().map(|m| m.chain.height()).min().unwrap();
        let at = |m: &Member| m.chain.active_chain().unwrap()[usize::try_from(common).unwrap()];
        let first = at(&self.members[0]);
        for (i, m) in self.members.iter().enumerate() {
            assert_eq!(
                at(m),
                first,
                "member {i} built a different block at {common}"
            );
        }
    }
}

#[test]
fn two_validators_restarted_into_follower_mode_keep_pace_across_an_epoch_boundary() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 3);

    // Member 2 is down while the chain advances, then restarts as a follower.
    mesh.hold_out(2);
    mesh.run_until(|m| m.height(0) >= 9);
    mesh.restart_from_certificates(2, 0);

    // Member 3 does the same while 2 follows: two followers, two builders.
    mesh.hold_out(3);
    mesh.run_until(|m| m.height(0) >= 15);
    mesh.restart_from_certificates(3, 0);

    let before: Vec<u64> = (0..VALIDATORS).map(|i| mesh.height(i)).collect();
    let target = mesh.height(0) + 12;
    mesh.run_until(|m| m.height(0) >= target);

    let after: Vec<u64> = (0..VALIDATORS).map(|i| mesh.height(i)).collect();
    for i in 0..VALIDATORS {
        assert!(
            after[i] > before[i],
            "member {i} stopped at {} (was {}); network at {}",
            after[i],
            before[i],
            mesh.height(0)
        );
    }
    // Twelve blocks is two epochs: the chain crossed at least one boundary.
    for i in 0..VALIDATORS {
        assert!(
            mesh.epoch(i).is_some_and(|e| e >= 2),
            "member {i} is not in epoch 2: {:?}",
            mesh.epoch(i)
        );
    }
    mesh.agree();
    assert_eq!(mesh.equivocations, 0);
}

/// Member `i`'s own certificates, counted from its log.
fn logged(mesh: &Mesh, i: usize, epoch: u64) -> usize {
    let mut count = 0;
    custom_l1_node::consensus::bft::store::for_each_certificate(
        &mesh.members[i].bft_dir,
        epoch,
        |_| {
            count += 1;
            Ok(())
        },
    )
    .unwrap();
    count
}

#[test]
fn forged_or_insufficient_evidence_is_refused_and_nothing_is_logged() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 6);
    mesh.hold_out(2);
    mesh.run_until(|m| m.height(0) >= 9);
    for forgery in [Forgery::BadSignature, Forgery::FewVotes] {
        mesh.reopen_chain(2);
        let before = logged(&mesh, 2, 1);
        let source = Forging {
            honest: mesh.peer_log(0),
            forgery,
        };
        let m = &mut mesh.members[2];
        let setup = &mesh.setups[2];
        let refused = certsync::sync(&mut m.chain, &source, setup, &m.bft_dir, mesh.now)
            .err()
            .expect("forged evidence must be refused");
        assert!(
            refused.to_string().contains("refused"),
            "{forgery:?}: {refused}"
        );
        assert_eq!(logged(&mesh, 2, 1), before, "{forgery:?} reached the log");
    }
}

#[test]
fn one_follower_plus_a_crashed_validator_halts_then_recovers_when_the_crashed_one_returns() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 4);
    // Member 3 has crashed and member 2 is catching up: two of four voting.
    mesh.hold_out(3);
    mesh.hold_out(2);
    mesh.run_ticks(20);
    let halted = mesh.height(0);
    mesh.run_ticks(60);
    assert_eq!(mesh.height(0), halted, "two of four must not advance the chain");

    // The crashed validator returns: three voting, the quorum is back.
    mesh.restart_from_certificates(3, 0);
    mesh.run_until(|m| m.height(0) >= halted + 3);
    assert!(mesh.height(0) >= halted + 3, "the quorum did not restore progress");

    // The follower catches up by replay and rejoins.
    mesh.restart_from_certificates(2, 0);
    let target = mesh.height(0) + 6;
    mesh.run_until(|m| m.height(0) >= target);
    for i in 0..VALIDATORS {
        assert!(mesh.height(i) + 3 >= target, "member {i} at {}", mesh.height(i));
    }
    mesh.agree();
    assert_eq!(mesh.equivocations, 0);
}

/// A peer's certificate log as a catch-up source, in process.
struct PeerLog {
    dir: PathBuf,
    epoch: u64,
}

impl CertSource for PeerLog {
    fn current_epoch(&self) -> custom_l1_node::error::Result<u64> {
        Ok(self.epoch)
    }

    fn page(
        &self,
        epoch: u64,
        offset: u64,
        max_records: usize,
    ) -> custom_l1_node::error::Result<CertPage> {
        read_certificate_page(&self.dir, epoch, offset, max_records, PAGE_BYTES)
    }
}

/// How a peer lies about the first certificate it serves.
#[derive(Clone, Copy, Debug)]
enum Forgery {
    BadSignature,
    FewVotes,
}

/// A peer that forges the first certificate of each page.
struct Forging {
    honest: PeerLog,
    forgery: Forgery,
}

impl CertSource for Forging {
    fn current_epoch(&self) -> custom_l1_node::error::Result<u64> {
        self.honest.current_epoch()
    }

    fn page(
        &self,
        epoch: u64,
        offset: u64,
        max_records: usize,
    ) -> custom_l1_node::error::Result<CertPage> {
        let mut page = self.honest.page(epoch, offset, max_records)?;
        if let Some(first) = page.records.first_mut() {
            let mut c = decode_certificate(first)?;
            match self.forgery {
                Forgery::BadSignature => c.signatures[0][0] ^= 0xff,
                Forgery::FewVotes => {
                    c.votes.pop();
                    c.signatures.pop();
                }
            }
            *first = encode_certificate(&c);
        }
        Ok(page)
    }
}
