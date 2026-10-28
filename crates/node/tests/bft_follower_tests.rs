//! Restarted validators catch up by certificate replay across an epoch boundary,
//! and a hostile peer cannot make them accept a shorter or forged history
//! (ADR-052, which supersedes the checkpoint follower of ADR-038 for this case).
//!
//! The 2026-10-18 incident on maya-testnet-1: two validators came back behind
//! and followed. Followers switch epochs only by importing the boundary block,
//! and a checkpoint needs `n - f` attesters they are not, so the chain halted at
//! the next epoch boundary. These tests pin that restarted validators replay
//! their way through, that the chain keeps advancing, and that forged, gapped,
//! truncated or stalling evidence is refused.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::ChainTag;
use std::cell::Cell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use custom_l1_node::consensus::bft::attest::Checkpoint;
use custom_l1_node::consensus::bft::auth::MlDsaAuthenticator;
use custom_l1_node::consensus::bft::certsync::{self, CertSource, PAGE, PageLimits};
use custom_l1_node::consensus::bft::store::for_each_certificate;
use custom_l1_node::consensus::bft::store::{CertPage, read_certificate_page};
use custom_l1_node::consensus::bft::wire::{decode_certificate, encode_certificate};
use custom_l1_node::consensus::bft::{BftDriver, BftSetup, Step};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::validator_keys::{self as keys, SigningKey, VerifyingKey};
use custom_l1_node::genesis::{Allocation, BftGenesis, GenesisBond, GenesisConfig, StakingGenesis};
use custom_l1_node::state::db::StateDB;
use maya_dag_bft::{Authenticator, Certificate, Params, SignContext, SignKind};
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

    /// Member `from`'s logs and checkpoints, as a catch-up source.
    fn peer_log(&self, from: usize) -> PeerLog {
        let driver = self.members[from].driver.as_ref().unwrap();
        let checkpoints = [driver.closing_checkpoint(), driver.checkpoint()]
            .into_iter()
            .flatten()
            .cloned()
            .collect();
        PeerLog {
            dir: self.members[from].bft_dir.clone(),
            epoch: driver.epoch(),
            checkpoints,
            limits: PAGE,
        }
    }

    /// Restarts member `i` the way an operator does with `--catch-up-from`:
    /// certificate replay from member `from` (ADR-052), then the engine. No
    /// checkpoint follower and no follower mode.
    fn restart_from_certificates(&mut self, i: usize, from: usize) {
        self.reopen_chain(i);
        let source = self.peer_log(from);
        self.restart_from(i, &source, PAGE).unwrap();
    }

    /// Replays member `i` from `source`, with `limits`, and installs its engine.
    fn restart_from(
        &mut self,
        i: usize,
        source: &impl CertSource,
        limits: PageLimits,
    ) -> custom_l1_node::error::Result<()> {
        let setup = &self.setups[i];
        let m = &mut self.members[i];
        let synced = certsync::sync(&mut m.chain, source, setup, &m.bft_dir, self.now, limits)?;
        m.driver = Some(synced.driver);
        self.absorb(i, synced.step);
        Ok(())
    }

    /// Runs `ticks` rounds of the clock without the usual stop condition.
    fn run_ticks(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.now += 250;
            self.tick();
            self.pump(3_000);
        }
    }

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
fn two_restarted_validators_replay_their_way_across_an_epoch_boundary() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 3);

    // Member 2 is down while the chain advances, then restarts by replay.
    mesh.hold_out(2);
    mesh.run_until(|m| m.height(0) >= 9);
    mesh.restart_from_certificates(2, 0);

    // Member 3 does the same while 2 is back: two restarted, two builders.
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

/// Member `i`'s own certificates in `epoch`, counted from its log.
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
        assert_eq!(mesh.epoch(0), Some(1), "the peer is in epoch 1");
        let before = logged(&mesh, 2, 1);
        let source = Forging {
            honest: mesh.peer_log(0),
            forgery,
        };
        let Err(refused) = mesh.restart_from(2, &source, PAGE) else {
            panic!("forged evidence must be refused");
        };
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
    assert_eq!(
        mesh.height(0),
        halted,
        "two of four must not advance the chain"
    );

    // The crashed validator returns: three voting, the quorum is back.
    mesh.restart_from_certificates(3, 0);
    mesh.run_until(|m| m.height(0) >= halted + 3);
    assert!(
        mesh.height(0) >= halted + 3,
        "the quorum did not restore progress"
    );

    // The follower catches up by replay and rejoins.
    mesh.restart_from_certificates(2, 0);
    let target = mesh.height(0) + 6;
    mesh.run_until(|m| m.height(0) >= target);
    for i in 0..VALIDATORS {
        assert!(
            mesh.height(i) + 3 >= target,
            "member {i} at {}",
            mesh.height(i)
        );
    }
    mesh.agree();
    assert_eq!(mesh.equivocations, 0);
}

#[test]
fn a_peer_that_ends_an_epoch_early_is_refused_by_the_attested_end() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 3);
    mesh.hold_out(2);
    mesh.run_until(|m| m.height(0) >= 8);
    mesh.reopen_chain(2);
    assert!(logged(&mesh, 0, 0) > 1, "the peer must still hold epoch 0");
    // Serves one certificate of epoch 0, then says the epoch is over.
    let honest = mesh.peer_log(0);
    let source = Truncating {
        checkpoints: honest.checkpoints.clone(),
        honest,
        keep: 1,
        only_epoch: None,
    };
    let Err(refused) = mesh.restart_from(2, &source, PAGE) else {
        panic!("a truncated epoch must be refused");
    };
    assert!(refused.to_string().contains("refused"), "{refused}");
}

#[test]
fn a_peer_that_skips_a_certificate_is_refused_by_the_attested_end() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 3);
    mesh.hold_out(2);
    mesh.run_until(|m| m.height(0) >= 8);
    mesh.reopen_chain(2);
    // Member 2 already holds epoch 0 from before its outage, which would hide
    // a gap. Its epoch-0 certificate log is removed so the fetch must bring
    // every certificate, and the peer leaves the first one out.
    std::fs::remove_file(mesh.members[2].bft_dir.join("epoch-0").join("certs.log")).unwrap();
    // A certificate nothing references leaves the history unchanged, and the
    // attested end is right to accept that. Skip one that a later certificate
    // names as a parent: its child then waits forever, and the replay stops.
    let mut logged_certs = Vec::new();
    custom_l1_node::consensus::bft::store::for_each_certificate(&mesh.members[0].bft_dir, 0, |c| {
        logged_certs.push(c);
        Ok(())
    })
    .unwrap();
    let named: std::collections::HashSet<_> = logged_certs
        .iter()
        .flat_map(|c| c.vertex.parents.iter().copied())
        .collect();
    let skip = logged_certs
        .iter()
        .position(|c| named.contains(&c.digest()))
        .unwrap_or_else(|| {
            let rounds: Vec<u64> = logged_certs.iter().map(|c| c.vertex.round).collect();
            let parents: Vec<usize> = logged_certs
                .iter()
                .map(|c| c.vertex.parents.len())
                .collect();
            panic!(
                "no parent among {} certs: rounds {rounds:?} parents {parents:?} named {}",
                logged_certs.len(),
                named.len()
            )
        });
    let source = Skipping {
        honest: mesh.peer_log(0),
        skip,
    };
    let Err(refused) = mesh.restart_from(2, &source, PAGE) else {
        panic!("a gapped epoch must be refused");
    };
    assert!(refused.to_string().contains("refused"), "{refused}");
}

#[test]
fn a_peer_that_stalls_the_fetch_is_refused_not_looped_on() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 6);
    mesh.hold_out(2);
    mesh.run_until(|m| m.height(0) >= 9);
    mesh.reopen_chain(2);
    let source = Stalling {
        honest: mesh.peer_log(0),
        calls: Cell::new(0),
    };
    let Err(refused) = mesh.restart_from(2, &source, PAGE) else {
        panic!("a stalled fetch must be refused");
    };
    // A page that repeats its records with an end that does not add up is
    // refused by the end check, before the no-progress check.
    assert!(refused.to_string().contains("does not match"), "{refused}");
}

#[test]
fn paging_runs_to_the_end_with_tiny_pages_and_stays_idempotent() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 3);
    mesh.hold_out(2);
    mesh.run_until(|m| m.height(0) >= 8);
    mesh.reopen_chain(2);
    // One record per page (the byte budget admits only the first record), so
    // every certificate arrives on its own page.
    let tiny = PageLimits {
        records: 2,
        encoded_bytes: 1,
    };
    let source = mesh.peer_log(0);
    mesh.restart_from(2, &source, tiny).unwrap();
    for epoch in 0..=1 {
        assert_eq!(
            logged(&mesh, 2, epoch),
            logged(&mesh, 0, epoch),
            "epoch {epoch}"
        );
    }
    let (height, tip) = (mesh.height(2), mesh.members[2].chain.tip());

    // A second catch-up from the same peer writes nothing and changes nothing.
    mesh.reopen_chain(2);
    let before = logged(&mesh, 2, 1);
    mesh.restart_from(2, &source, PAGE).unwrap();
    assert_eq!(logged(&mesh, 2, 1), before, "a repeat wrote certificates");
    assert_eq!(mesh.height(2), height);
    assert_eq!(mesh.members[2].chain.tip(), tip);
}

/// A peer's logs and checkpoints as a catch-up source, in process.
struct PeerLog {
    dir: PathBuf,
    epoch: u64,
    checkpoints: Vec<Checkpoint>,
    limits: PageLimits,
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
        read_certificate_page(
            &self.dir,
            epoch,
            offset,
            max_records.min(self.limits.records),
            self.limits.encoded_bytes,
        )
    }

    fn checkpoint_of(&self, epoch: u64) -> custom_l1_node::error::Result<Option<Checkpoint>> {
        Ok(self
            .checkpoints
            .iter()
            .filter(|c| c.epoch == epoch)
            .max_by_key(|c| c.height)
            .cloned())
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

    fn checkpoint_of(&self, epoch: u64) -> custom_l1_node::error::Result<Option<Checkpoint>> {
        self.honest.checkpoint_of(epoch)
    }
}

/// Serves only the first `keep` records of the first page of `only_epoch` (of
/// every epoch when `None`), then says the epoch is over. Every page it sends is
/// well formed, and the checkpoints it offers are its own, so only the attested
/// end can tell.
struct Truncating {
    honest: PeerLog,
    keep: usize,
    only_epoch: Option<u64>,
    checkpoints: Vec<Checkpoint>,
}

impl CertSource for Truncating {
    fn current_epoch(&self) -> custom_l1_node::error::Result<u64> {
        self.honest.current_epoch()
    }

    fn page(
        &self,
        epoch: u64,
        offset: u64,
        max_records: usize,
    ) -> custom_l1_node::error::Result<CertPage> {
        if offset != 0 {
            return Ok(CertPage {
                records: Vec::new(),
                next: offset,
                done: true,
            });
        }
        let mut page = self.honest.page(epoch, 0, max_records)?;
        if self.only_epoch.is_some_and(|e| e != epoch) {
            return Ok(page);
        }
        page.records.truncate(self.keep);
        page.next = page
            .records
            .iter()
            .fold(0_u64, |at, r| at + 8 + u64::try_from(r.len()).unwrap());
        page.done = true;
        Ok(page)
    }

    fn checkpoint_of(&self, epoch: u64) -> custom_l1_node::error::Result<Option<Checkpoint>> {
        Ok(self
            .checkpoints
            .iter()
            .filter(|c| c.epoch == epoch)
            .max_by_key(|c| c.height)
            .cloned())
    }
}

/// Leaves record `skip` out of the first page and recomputes the page's end, so
/// the page is internally consistent: only the attested end can tell.
struct Skipping {
    honest: PeerLog,
    skip: usize,
}

impl CertSource for Skipping {
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
        if offset == 0 && self.skip < page.records.len() {
            page.records.remove(self.skip);
            page.next = page
                .records
                .iter()
                .fold(0_u64, |at, r| at + 8 + u64::try_from(r.len()).unwrap());
        }
        Ok(page)
    }

    fn checkpoint_of(&self, epoch: u64) -> custom_l1_node::error::Result<Option<Checkpoint>> {
        self.honest.checkpoint_of(epoch)
    }
}

/// Every certificate in `dir`'s epoch-`epoch` log, in log order.
fn logged_certs(dir: &Path, epoch: u64) -> Vec<Certificate> {
    let mut certs = Vec::new();
    for_each_certificate(dir, epoch, |c| {
        certs.push(c);
        Ok(())
    })
    .unwrap();
    certs
}

/// The encoded copies of one slot in `dir`'s epoch-`epoch` log.
fn slot_copies(dir: &Path, epoch: u64, slot: (u64, u16)) -> Vec<Vec<u8>> {
    logged_certs(dir, epoch)
        .iter()
        .filter(|c| (c.vertex.round, c.vertex.author) == slot)
        .map(encode_certificate)
        .collect()
}

/// A fresh observer that holds nothing: the node that catches up from scratch.
fn observer(mesh: &Mesh) -> (TempDir, Chain, BftSetup) {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(StateDB::open(dir.path().join("state")).unwrap());
    mesh.config.seed_state(&state).unwrap();
    let chain = common::open_chain(
        state,
        mesh.config.genesis_block().unwrap(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap();
    let setup = BftSetup {
        epoch: 0,
        committee: Arc::clone(&mesh.setups[0].committee),
        signer: None,
        params: mesh.setups[0].params,
    };
    (dir, chain, setup)
}

/// `c` with a later timestamp and votes re-signed by the real validator keys:
/// a different vertex for the same slot, every signature valid.
fn resigned(c: &Certificate, mesh: &Mesh) -> Certificate {
    let mut alt = c.clone();
    alt.vertex.timestamp_ms += 1;
    let digest = alt.digest();
    let (round, author) = (alt.vertex.round, alt.vertex.author);
    let committee = Arc::clone(&mesh.setups[0].committee);
    alt.signatures = alt
        .votes
        .iter()
        .map(|&voter| {
            MlDsaAuthenticator::validator(
                validator_key(usize::from(voter)),
                Arc::clone(&committee),
                ChainTag::from_genesis(mesh.members[0].chain.genesis()),
            )
            .sign(
                SignContext {
                    kind: SignKind::Vote,
                    round,
                    author,
                },
                &digest,
            )
        })
        .collect();
    alt
}

#[test]
fn a_peer_that_serves_an_older_checkpoint_with_a_truncated_log_is_refused() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 4);
    let older = mesh.members[0]
        .driver
        .as_ref()
        .unwrap()
        .checkpoint()
        .cloned()
        .expect("a checkpoint by height 4");
    mesh.run_until(|m| m.height(0) >= 8);
    let total = logged_certs(&mesh.members[0].bft_dir, 0).len();
    // Every truncation point, each with the peer's older valid checkpoint.
    let mut accepted = Vec::new();
    for keep in 0..=total {
        let (dir, mut chain, setup) = observer(&mesh);
        let source = Truncating {
            honest: mesh.peer_log(0),
            keep,
            only_epoch: None,
            checkpoints: vec![older.clone()],
        };
        if let Ok(synced) = certsync::sync(
            &mut chain,
            &source,
            &setup,
            &dir.path().join("bft"),
            0,
            PAGE,
        ) {
            accepted.push((keep, synced.crossed));
        }
    }
    assert!(
        accepted.is_empty(),
        "truncated logs with an older checkpoint were accepted as (keep, crossed): {accepted:?}"
    );
}

#[test]
fn a_checkpoint_newer_than_the_fetched_logs_does_not_refuse_the_head_epoch() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 8);
    let honest = mesh.peer_log(0);
    let checkpoints = honest.checkpoints.clone();
    let head_checkpoint = checkpoints
        .iter()
        .filter(|c| c.epoch == 1)
        .map(|c| c.height)
        .max()
        .expect("the head epoch has a checkpoint");
    let (dir, mut chain, setup) = observer(&mesh);
    // Epoch 0 whole. Epoch 1, the head, is served for one record only, so the
    // replay stops short of the checkpoint the peer holds.
    let source = Truncating {
        honest,
        keep: 1,
        only_epoch: Some(1),
        checkpoints,
    };
    let synced = certsync::sync(
        &mut chain,
        &source,
        &setup,
        &dir.path().join("bft"),
        0,
        PAGE,
    )
    .expect("the head epoch may be ahead of the logs fetched so far");
    assert!(
        chain.height() < head_checkpoint,
        "the scenario needs the replay short of the checkpoint ({} vs {head_checkpoint})",
        chain.height()
    );
    assert_eq!(synced.crossed, 1, "the boundary was still crossed");
}

/// Serves the peer's page with record `index` replaced by `record`, the rest
/// untouched. The replacement is a valid certificate for the same slot.
struct Substituting {
    honest: PeerLog,
    index: usize,
    record: Vec<u8>,
}

impl CertSource for Substituting {
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
        if epoch == 0 && offset == 0 && self.index < page.records.len() {
            page.records[self.index] = self.record.clone();
            page.next = page
                .records
                .iter()
                .fold(0_u64, |at, r| at + 8 + u64::try_from(r.len()).unwrap());
        }
        Ok(page)
    }

    fn checkpoint_of(&self, epoch: u64) -> custom_l1_node::error::Result<Option<Checkpoint>> {
        self.honest.checkpoint_of(epoch)
    }
}

#[test]
fn a_valid_equivocating_copy_of_a_logged_slot_is_counted_and_does_not_abort_the_sync() {
    let mut mesh = Mesh::new();
    mesh.run_until(|m| m.height(0) >= 3);
    // Member 2 keeps its epoch-0 log from before its outage.
    mesh.hold_out(2);
    mesh.run_until(|m| m.height(0) >= 5);
    mesh.reopen_chain(2);
    let mine = logged_certs(&mesh.members[2].bft_dir, 0)[0].clone();
    let slot = (mine.vertex.round, mine.vertex.author);
    let peer = mesh.peer_log(0);
    let index = logged_certs(&peer.dir, 0)
        .iter()
        .position(|c| (c.vertex.round, c.vertex.author) == slot)
        .expect("the peer holds the slot");
    let alt = resigned(&mine, &mesh);
    assert_ne!(encode_certificate(&alt), encode_certificate(&mine));
    let before = slot_copies(&mesh.members[2].bft_dir, 0, slot);
    assert_eq!(before.len(), 1, "one copy of the slot to start with");

    let source = Substituting {
        honest: peer,
        index,
        record: encode_certificate(&alt),
    };
    let setup = &mesh.setups[2];
    let m = &mut mesh.members[2];
    let synced = certsync::sync(&mut m.chain, &source, setup, &m.bft_dir, 0, PAGE)
        .expect("an equivocating copy must not abort the sync");
    assert_eq!(synced.equivocations, 1, "counted as evidence");
    assert_eq!(
        slot_copies(&mesh.members[2].bft_dir, 0, slot),
        before,
        "the first copy stays in the log"
    );
}

/// Returns the same non-empty page forever, claiming it is not the last.
struct Stalling {
    honest: PeerLog,
    calls: Cell<u32>,
}

impl CertSource for Stalling {
    fn current_epoch(&self) -> custom_l1_node::error::Result<u64> {
        self.honest.current_epoch()
    }

    fn page(
        &self,
        epoch: u64,
        offset: u64,
        max_records: usize,
    ) -> custom_l1_node::error::Result<CertPage> {
        self.calls.set(self.calls.get() + 1);
        assert!(self.calls.get() < 1_000, "the fetch did not stop");
        let page = self.honest.page(epoch, offset, max_records)?;
        Ok(CertPage {
            records: page.records,
            next: offset,
            done: false,
        })
    }

    fn checkpoint_of(&self, epoch: u64) -> custom_l1_node::error::Result<Option<Checkpoint>> {
        self.honest.checkpoint_of(epoch)
    }
}
