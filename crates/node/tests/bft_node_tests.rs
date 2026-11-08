//! DAG-BFT run by the node's own driver: four ML-DSA-65 validators and one
//! observer, each on its own `RocksDB` state and chain, exchanging real wire
//! frames through an in-memory mesh (ADR-027).
//!
//! What these pin, end to end through the code the binary runs:
//!
//! - every node builds the same blocks from the same certificates — same tip
//!   id, same state root — with no block ever sent between them;
//! - a submitted transfer executes on all of them;
//! - a double spend proposed by two validators lands once;
//! - an observer with no key follows to the same tip;
//! - a validator restarted from its safety log rejoins without equivocating.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;

use custom_l1_node::consensus::bft::attest::{ATTEST_FRAME, Attestation, CheckpointBook};
use custom_l1_node::consensus::bft::{BftDriver, BftSetup, Step};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::crypto::validator_keys::{self as keys, SigningKey, VerifyingKey};
use custom_l1_node::genesis::GenesisConfig;
use custom_l1_node::state::account::Account;
use custom_l1_node::state::db::StateDB;
use custom_l1_node::upgrade::{FeatureSet, ProtocolUpgrade, SUPPORTED_PROTOCOL_VERSION};
use maya_dag_bft::Params;
use tempfile::TempDir;

mod common;

const VALIDATORS: usize = 4;
const RECIPIENT: [u8; 32] = [0x77; 32];
const START_BALANCE: u64 = 1_000_000;

fn genesis() -> Block {
    Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

fn user() -> HybridSigningKey {
    signing_key_from_seed(&[0x55; 32]).unwrap()
}

fn transfer(amount: u64, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: primitive_types::U256::from(amount),
            recipient: RECIPIENT,
        }],
        nonce,
    );
    tx.sign(&user(), &common::test_chain()).unwrap();
    tx
}

struct Member {
    driver: Option<BftDriver>,
    chain: Chain,
    bft_dir: PathBuf,
    state_dir: PathBuf,
    /// How far this member's clock runs ahead, for the clock-drift rehearsal.
    skew_ms: u64,
}

struct Mesh {
    _root: TempDir,
    members: Vec<Member>,
    setups: Vec<BftSetup>,
    queue: VecDeque<(usize, Vec<u8>)>,
    now: u64,
    equivocations: usize,
    /// Members running a binary older than the schedule (`Mesh::with_old`).
    old: Vec<usize>,
    /// Every step an old member produced, in order: its frames and notices.
    old_steps: Vec<(usize, Vec<Vec<u8>>, Vec<String>)>,
}

fn signers() -> (Vec<Arc<SigningKey>>, Arc<[VerifyingKey]>) {
    let keys: Vec<Arc<SigningKey>> = (0..VALIDATORS)
        .map(|i| {
            Arc::new(keys::signing_key_from_seed(&[u8::try_from(i).unwrap() + 1; 32]).unwrap())
        })
        .collect();
    let committee: Arc<[VerifyingKey]> = keys.iter().map(|k| k.verifying_key()).collect();
    (keys, committee)
}

fn open_chain(state_dir: &PathBuf, fund: bool) -> Chain {
    let state = Arc::new(StateDB::open(state_dir).unwrap());
    if fund {
        state
            .put_account(
                &user().address(),
                &Account {
                    balance: primitive_types::U256::from(START_BALANCE),
                    nonce: 0,
                },
            )
            .unwrap();
    }
    common::open_chain(state, genesis(), ChainConfig::dag_bft()).unwrap()
}

/// The height a version newer than this binary activates at, for an old member.
const HALT_AT: u64 = 4;

/// Writes a genesis schedule entry for a version this binary does not
/// implement, before the chain opens. One binary cannot run as two, so an
/// "old binary" is a member whose schedule names a version above
/// `SUPPORTED_PROTOCOL_VERSION`; the halt makes that same comparison. Its state
/// root therefore differs from the upgraded members', so its blocks below the
/// halt are its own, which the DAG does not care about.
fn seed_newer_version(state_dir: &PathBuf) {
    let config = GenesisConfig {
        chain_id: "bft-upgrade-halt".to_string(),
        timestamp: 1_000_000,
        difficulty_bits: 0,
        pow_limit_bits: 0,
        allocations: Vec::new(),
        oracle: None,
        sealed: None,
        treasury: None,
        protocol_upgrades: vec![ProtocolUpgrade {
            version: SUPPORTED_PROTOCOL_VERSION + 1,
            activation_height: HALT_AT,
            features: FeatureSet::default(),
        }],
        suite_deprecations: Vec::new(),
        security_council: None,
        shielded_activation_height: None,
        bft: None,
    };
    let state = StateDB::open(state_dir).unwrap();
    for (key, value) in config.seeded_records().unwrap() {
        state.raw_put_for_test(&key, &value).unwrap();
    }
}

impl Mesh {
    /// Four validators and `observers` keyless followers.
    fn new(observers: usize) -> Self {
        Self::with_old(observers, &[])
    }

    /// As [`Mesh::new`], with the validators in `old` running a binary that
    /// does not implement the version scheduled at [`HALT_AT`].
    fn with_old(observers: usize, old: &[usize]) -> Self {
        let root = TempDir::new().unwrap();
        let (keys, committee) = signers();
        let params = Params {
            batch_size: 100,
            anchor_timeout_ms: 1_000,
            // What `maya2c-node` derives (anchor timeout / 2). The default
            // 0 is the simulator's: a lagging engine then re-fetches on every
            // duplicate certificate and peers answer without a budget, and
            // the fetch storm buried this mesh's FIFO for good (2026-11-08).
            resend_interval_ms: 500,
            ..Params::default()
        };
        let mut setups = Vec::new();
        let mut members = Vec::new();
        let mut opening = Vec::new();
        for i in 0..VALIDATORS + observers {
            let setup = BftSetup {
                epoch: 0,
                committee: Arc::clone(&committee),
                signer: keys.get(i).cloned().map(Into::into),
                params,
            };
            let state_dir = root.path().join(format!("state-{i}"));
            let bft_dir = root.path().join(format!("bft-{i}"));
            if old.contains(&i) {
                seed_newer_version(&state_dir);
            }
            let mut chain = open_chain(&state_dir, true);
            let (driver, step) = BftDriver::open(&setup, &bft_dir, &mut chain, 0).unwrap();
            members.push(Member {
                driver: Some(driver),
                skew_ms: 0,
                chain,
                bft_dir,
                state_dir,
            });
            setups.push(setup);
            opening.push((i, step));
        }
        let mut mesh = Self {
            _root: root,
            members,
            setups,
            queue: VecDeque::new(),
            now: 0,
            equivocations: 0,
            old: old.to_vec(),
            old_steps: Vec::new(),
        };
        for (i, step) in opening {
            mesh.absorb(i, step);
        }
        mesh
    }

    fn absorb(&mut self, from: usize, step: Step) {
        self.equivocations += step.equivocations.len();
        if self.old.contains(&from) {
            self.old_steps
                .push((from, step.frames.clone(), step.notices.clone()));
        }
        // What the binary's feed does: ordered-but-dropped transactions go
        // back to the validator that proposed them.
        if let Some(driver) = self.members[from].driver.as_mut() {
            for tx in &step.dropped {
                driver.submit(tx);
            }
        }
        self.queue
            .extend(step.frames.into_iter().map(|f| (from, f)));
    }

    fn tick(&mut self) {
        for i in 0..self.members.len() {
            let m = &mut self.members[i];
            let Some(driver) = m.driver.as_mut() else {
                continue;
            };
            let step = driver.on_tick(&mut m.chain, self.now + m.skew_ms).unwrap();
            self.absorb(i, step);
        }
    }

    /// Delivers up to `budget` frames, each to every other live member.
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
                let step = driver
                    .on_frame(&mut m.chain, self.now + m.skew_ms, &frame)
                    .unwrap();
                self.absorb(i, step);
            }
        }
    }

    /// Runs until every live member is at `height` or the budget is spent.
    fn run_to(&mut self, height: u64) {
        let live: Vec<usize> = (0..self.members.len())
            .filter(|i| self.members[*i].driver.is_some())
            .collect();
        self.run_to_among(&live, height);
    }

    /// Runs until every member in `who` is at `height` or the budget is spent.
    fn run_to_among(&mut self, who: &[usize], height: u64) {
        for _ in 0..400 {
            if who
                .iter()
                .all(|i| self.members[*i].chain.height() >= height)
            {
                return;
            }
            self.rounds(1);
        }
    }

    /// `n` quarter-second ticks, each followed by delivery.
    fn rounds(&mut self, n: usize) {
        for _ in 0..n {
            self.now += 250;
            self.tick();
            self.pump(2_000);
        }
    }

    /// Asserts old member `who` said once that it halted, and from that step
    /// on sent nothing but attestations of heights below the halt — no
    /// proposal, vote or certificate, and nothing at all after that step.
    fn assert_went_silent_at_halt(&self, who: usize) {
        let steps: Vec<_> = self.old_steps.iter().filter(|s| s.0 == who).collect();
        let marker = format!("upgrade required before height {HALT_AT}");
        let halts: Vec<usize> = steps
            .iter()
            .enumerate()
            .filter(|(_, s)| s.2.iter().any(|n| n.contains(&marker)))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(halts.len(), 1, "member {who} logs the halt exactly once");
        for frame in steps.iter().flat_map(|s| &s.1) {
            if frame.first() == Some(&ATTEST_FRAME) {
                let height = Attestation::decode(frame).unwrap().height;
                assert!(height < HALT_AT, "member {who} attested height {height}");
            }
        }
        for frame in &steps[halts[0]].1 {
            assert_eq!(
                frame.first(),
                Some(&ATTEST_FRAME),
                "member {who} sent a DAG frame in its halt step"
            );
        }
        let after: usize = steps[halts[0] + 1..].iter().map(|s| s.1.len()).sum();
        assert_eq!(after, 0, "member {who} sent {after} frame(s) after halting");
    }

    /// The members in `who` hold one block at every height up to the lowest
    /// of their tips. Returns that height.
    fn agree_among(&self, who: &[usize]) -> u64 {
        let common = who
            .iter()
            .map(|i| self.members[*i].chain.height())
            .min()
            .unwrap();
        let chains: Vec<Vec<[u8; 32]>> = who
            .iter()
            .map(|i| self.members[*i].chain.active_chain().unwrap())
            .collect();
        for h in 0..=usize::try_from(common).unwrap() {
            assert!(chains.iter().all(|c| c[h] == chains[0][h]), "fork at {h}");
        }
        common
    }

    fn live(&self) -> impl Iterator<Item = &Member> {
        self.members.iter().filter(|m| m.driver.is_some())
    }

    fn submit(&mut self, validator: usize, tx: &Transaction) {
        self.members[validator].driver.as_mut().unwrap().submit(tx);
    }

    /// Every live member's block at `height` has the same id.
    fn agree_at(&self, height: u64) -> [u8; 32] {
        let ids: Vec<[u8; 32]> = self
            .live()
            .map(|m| {
                let chain = m.chain.active_chain().unwrap();
                chain[usize::try_from(height).unwrap()]
            })
            .collect();
        for id in &ids {
            assert_eq!(id, &ids[0], "members built different blocks at {height}");
        }
        ids[0]
    }

    fn balance(&self, i: usize, who: &[u8; 32]) -> u64 {
        self.members[i]
            .chain
            .state()
            .get_account(who)
            .unwrap()
            .balance
            .try_into()
            .expect("fits u64")
    }
}

#[test]
fn four_validators_and_an_observer_build_identical_chains_and_execute_a_transfer() {
    let mut mesh = Mesh::new(1);
    mesh.submit(0, &transfer(250, 0));
    mesh.run_to(6);
    let heights: Vec<u64> = mesh.live().map(|m| m.chain.height()).collect();
    assert!(heights.iter().all(|h| *h >= 6), "heights {heights:?}");
    let common = *heights.iter().min().unwrap();
    mesh.agree_at(common);
    for i in 0..mesh.members.len() {
        assert_eq!(mesh.balance(i, &RECIPIENT), 250, "member {i}");
    }
    let roots: Vec<[u8; 32]> = mesh
        .live()
        .map(|m| m.chain.state().state_root().unwrap())
        .collect();
    // Members may be a block apart; compare at the common height's tip only
    // when all are there.
    if heights.iter().all(|h| *h == common) {
        assert!(roots.iter().all(|r| *r == roots[0]));
    }
    assert_eq!(mesh.equivocations, 0);
}

#[test]
fn a_double_spend_proposed_by_two_validators_lands_once() {
    let mut mesh = Mesh::new(0);
    // Same nonce, different amounts: two valid transactions, one spend.
    mesh.submit(0, &transfer(100, 0));
    mesh.submit(1, &transfer(300, 0));
    mesh.run_to(6);
    let common = mesh.live().map(|m| m.chain.height()).min().unwrap();
    mesh.agree_at(common);
    let got: Vec<u64> = (0..VALIDATORS)
        .map(|i| mesh.balance(i, &RECIPIENT))
        .collect();
    assert!(got[0] == 100 || got[0] == 300, "recipient got {}", got[0]);
    assert!(
        got.iter().all(|g| *g == got[0]),
        "members disagree: {got:?}"
    );
    let spender: Vec<u64> = (0..VALIDATORS)
        .map(|i| mesh.balance(i, &user().address()))
        .collect();
    assert!(spender.iter().all(|b| *b == START_BALANCE - got[0]));
}

#[test]
fn a_validator_restarted_from_its_safety_log_rejoins_without_equivocating() {
    let mut mesh = Mesh::new(0);
    mesh.run_to(3);
    // Stop validator 2 mid-flight and let the other three continue (3 = 2f+1).
    mesh.members[2].driver = None;
    let before = mesh.members[0].chain.height();
    mesh.run_to(before + 3);
    assert!(
        mesh.members[0].chain.height() >= before + 3,
        "3 of 4 stalled"
    );
    // Restart it from disk: same state, same safety log.
    let m = &mut mesh.members[2];
    // The old handle holds RocksDB's lock; swap it out before reopening.
    m.chain = open_chain(&m.state_dir.with_extension("scratch"), false);
    m.chain = open_chain(&m.state_dir, false);
    let (driver, step) =
        BftDriver::open(&mesh.setups[2], &m.bft_dir, &mut m.chain, mesh.now).unwrap();
    m.driver = Some(driver);
    mesh.absorb(2, step);
    let target = mesh.members[0].chain.height() + 4;
    mesh.run_to(target);
    let common = mesh.live().map(|m| m.chain.height()).min().unwrap();
    assert!(
        mesh.members[2].chain.height() >= before + 3,
        "restarted validator did not catch up: {}",
        mesh.members[2].chain.height()
    );
    mesh.agree_at(common);
    assert_eq!(
        mesh.equivocations, 0,
        "the restarted validator signed a slot twice"
    );
}

/// ADR-082: the launch binary is the one that will be old at the first
/// upgrade, so what it does at the halt height is fixed for good. It stops at
/// `HALT_AT - 1`, says so once, signs nothing more, and idles; the three
/// upgraded validators (2f+1) keep finalizing without it.
#[test]
fn a_validator_without_the_scheduled_version_goes_silent_at_its_height_and_the_rest_carry_on() {
    let mut mesh = Mesh::with_old(0, &[3]);
    mesh.run_to_among(&[0, 1, 2], HALT_AT + 3);

    assert_eq!(mesh.members[3].chain.height(), HALT_AT - 1);
    let common = mesh.agree_among(&[0, 1, 2]);
    assert!(
        common >= HALT_AT + 3,
        "the upgraded 3 of 4 stalled at {common}"
    );
    mesh.assert_went_silent_at_halt(3);
    assert_eq!(mesh.equivocations, 0, "the halted validator left evidence");

    // Idle, not spinning: the engine is never fed again, so its round stays
    // put and a tick or a frame returns at once with nothing to send.
    let driver = mesh.members[3].driver.as_ref().unwrap();
    let round = driver.round();
    let steps = mesh.old_steps.len();
    mesh.rounds(20);
    assert_eq!(mesh.members[3].driver.as_ref().unwrap().round(), round);
    assert!(
        mesh.old_steps[steps..]
            .iter()
            .all(|s| s.1.is_empty() && s.2.is_empty())
    );
    assert_eq!(mesh.members[3].chain.height(), HALT_AT - 1);
}

/// More than f validators without the version: finality stops at the halt
/// for everyone, and nobody forks. The two upgraded members may finish the
/// anchors already certified, then stall together on one chain.
#[test]
fn more_than_f_validators_without_the_scheduled_version_stall_the_chain_without_a_fork() {
    let mut mesh = Mesh::with_old(0, &[2, 3]);
    mesh.rounds(60);
    let stalled = mesh.agree_among(&[0, 1]);
    mesh.rounds(40);

    assert_eq!(
        mesh.agree_among(&[0, 1]),
        stalled,
        "the chain kept finalizing"
    );
    assert!(mesh.members[0].chain.height() <= stalled + 1);
    for old in [2, 3] {
        assert_eq!(mesh.members[old].chain.height(), HALT_AT - 1);
        mesh.assert_went_silent_at_halt(old);
    }
    assert_eq!(mesh.equivocations, 0);
    println!(
        "upgrade halt with 2 of 4 old: upgraded members stalled at height {stalled} (halt at {HALT_AT})"
    );
}

#[test]
fn a_nonce_chain_split_across_validators_all_lands() {
    let mut mesh = Mesh::new(0);
    // Nonce 1 goes to a validator that will likely order it before nonce 0
    // reaches the chain; it must be re-proposed, not stranded.
    mesh.submit(0, &transfer(10, 1));
    mesh.submit(1, &transfer(10, 0));
    mesh.submit(2, &transfer(10, 2));
    mesh.run_to(12);
    let common = mesh.live().map(|m| m.chain.height()).min().unwrap();
    mesh.agree_at(common);
    for i in 0..VALIDATORS {
        assert_eq!(mesh.balance(i, &RECIPIENT), 30, "member {i}");
    }
}

/// Runbook `clock-drift`: one validator's clock runs ten minutes fast.
///
/// What the runbook needs to know is what it breaks: liveness does not
/// depend on agreement between clocks (the engine's only use of time is its
/// own timeout), so the chain keeps finalizing. What drifts is block
/// timestamps — a fast leader's anchors carry its clock, and the builder only
/// clamps them to never run backwards. The size of that distortion is the
/// rehearsal's number.
#[test]
fn clock_drift_rehearsal_a_fast_validator_skews_timestamps_but_not_liveness() {
    const SKEW_MS: u64 = 10 * 60 * 1_000;
    let mut mesh = Mesh::new(0);
    // Engine clocks at the genesis time, as real ones would be; a few honest
    // rounds first, then one validator's clock jumps ten minutes ahead.
    mesh.now = 1_000_000 * 1_000;
    mesh.run_to(3);
    mesh.members[1].skew_ms = SKEW_MS;
    let start_s = mesh.now / 1_000;
    mesh.run_to(15);
    let common = mesh.live().map(|m| m.chain.height()).min().unwrap();
    assert!(
        common >= 12,
        "the chain stalled with a skewed validator: {common}"
    );
    mesh.agree_at(common);
    let chain = &mesh.members[0].chain;
    let stamps: Vec<u64> = chain
        .active_chain()
        .unwrap()
        .iter()
        .map(|id| chain.get(id).unwrap().header.timestamp)
        .collect();
    assert!(
        stamps.windows(2).all(|w| w[1] >= w[0]),
        "timestamps ran backwards"
    );
    let ahead = stamps.last().unwrap().saturating_sub(mesh.now / 1_000);
    assert!(
        ahead <= 1,
        "one fast clock moved the median block time {ahead} s ahead"
    );
    println!(
        "clock-drift: one validator 600 s fast; {common} blocks finalized and agreed in {} s of honest time;          timestamps monotonic; the chain's clock ends {ahead} s ahead of the honest one (median of certified times):          liveness and block time both unaffected",
        mesh.now / 1_000 - start_s
    );
}

#[test]
fn every_member_holds_a_quorum_attested_checkpoint_on_its_own_chain() {
    // ADR-038: validators attest each block they build; 3 of 4 make a
    // checkpoint every member (the observer too) can verify on its own.
    let mut mesh = Mesh::new(1);
    mesh.run_to(8);
    let (_, committee) = signers();
    for m in mesh.live() {
        let driver = m.driver.as_ref().unwrap();
        let checkpoint = driver
            .checkpoint()
            .expect("a checkpoint after eight blocks");
        let tag = custom_l1_node::core::ChainTag::from_genesis(m.chain.genesis());
        checkpoint
            .verify(&tag, &committee, None)
            .expect("a quorum of the committee");
        assert!(checkpoint.height >= 1);
        let ours = m.chain.active_chain().unwrap()[usize::try_from(checkpoint.height).unwrap()];
        assert_eq!(
            ours, checkpoint.block,
            "the checkpoint is this member's own block"
        );
    }
    assert_eq!(mesh.equivocations, 0);
}

/// A peer as a catch-up source: its newest checkpoint and its own blocks.
struct Peer<'a> {
    chain: &'a Chain,
    checkpoint: Option<custom_l1_node::consensus::bft::attest::Checkpoint>,
    /// The tip it reports: its chain height, or less for a follower that trails.
    tip: u64,
}

impl custom_l1_node::consensus::bft::catchup::CheckpointSource for Peer<'_> {
    fn checkpoint(
        &self,
    ) -> custom_l1_node::error::Result<Option<custom_l1_node::consensus::bft::attest::Checkpoint>>
    {
        Ok(self.checkpoint.clone())
    }

    fn checkpoint_of(
        &self,
        epoch: u64,
    ) -> custom_l1_node::error::Result<Option<custom_l1_node::consensus::bft::attest::Checkpoint>>
    {
        Ok(self.checkpoint.clone().filter(|c| c.epoch == epoch))
    }

    fn block(&self, height: u64) -> custom_l1_node::error::Result<Block> {
        let state = self.chain.state();
        let id = state.canonical_id(height)?.expect("peer holds the height");
        Ok(state.load_block(&id)?.expect("peer holds the body"))
    }

    fn tip_height(&self) -> custom_l1_node::error::Result<u64> {
        Ok(self.tip)
    }
}

#[test]
fn a_validator_down_far_past_the_engine_window_rejoins_through_a_checkpoint() {
    // ADR-038, mainnet gate 8. The 2026-10-04 dry run stranded a validator
    // for good after 90 s down: the engine keeps 50 rounds, and nodes never
    // import blocks they did not derive. Here it is held out for far longer,
    // catches up to a peer's checkpoint, and rejoins as an attested
    // follower: voting and proposing, importing blocks rather than building.
    let mut mesh = Mesh::new(0);
    mesh.run_to(3);
    mesh.members[2].driver = None;
    let before = mesh.members[2].chain.height();
    // 60 blocks is well over 100 rounds: twice the engine's 50-round window.
    mesh.run_to(before + 60);
    assert!(
        mesh.members[0].chain.height() >= before + 60,
        "3 of 4 stalled"
    );

    // Reopened from disk, as after an outage.
    let m = &mut mesh.members[2];
    m.chain = open_chain(&m.state_dir.with_extension("scratch"), false);
    m.chain = open_chain(&m.state_dir, false);
    let (_, committee) = signers();
    let imported = catch_up_member(&mut mesh, 2, 0, &committee);
    assert!(imported >= 50, "imported {imported}");

    let m = &mut mesh.members[2];
    let (mut driver, step) =
        BftDriver::open(&mesh.setups[2], &m.bft_dir, &mut m.chain, mesh.now).unwrap();
    driver.follow_attested(&m.chain);
    m.driver = Some(driver);
    mesh.absorb(2, step);

    let start = mesh.members[0].chain.height();
    for _ in 0..30 {
        mesh.now += 250;
        mesh.tick();
        mesh.pump(2_000);
        catch_up_member(&mut mesh, 2, 0, &committee);
    }
    let (lead, follower) = (
        mesh.members[0].chain.height(),
        mesh.members[2].chain.height(),
    );
    assert!(lead > start + 5, "the network kept going");
    assert!(
        lead - follower <= 3,
        "the follower keeps pace: {follower} vs {lead}"
    );
    mesh.agree_at(follower);
    let (r0, r2) = (
        mesh.members[0].driver.as_ref().unwrap().round(),
        mesh.members[2].driver.as_ref().unwrap().round(),
    );
    assert!(
        r0.abs_diff(r2) <= 4,
        "the follower proposes in current rounds: {r2} vs {r0}"
    );
    assert_eq!(mesh.equivocations, 0);
}

#[test]
fn a_validator_restarted_sixty_blocks_behind_builds_again_from_a_live_source() {
    // Gate 10: a follower only imports up to a checkpoint, which trails the
    // network's tip, so a restarted validator that never builds again stays
    // a follower for good, and its attestations never count. Here its source
    // is a builder's newest checkpoint, as on a real network.
    const INTERVALS: usize = 40;
    let mut mesh = Mesh::new(0);
    mesh.run_to(3);
    mesh.members[2].driver = None;
    let before = mesh.members[2].chain.height();
    mesh.run_to(before + 60);
    let m = &mut mesh.members[2];
    m.chain = open_chain(&m.state_dir.with_extension("scratch"), false);
    m.chain = open_chain(&m.state_dir, false);
    let (_, committee) = signers();
    assert!(catch_up_member(&mut mesh, 2, 0, &committee) >= 50);
    open_member_two(&mut mesh);
    let m = &mut mesh.members[2];
    m.driver.as_mut().unwrap().follow_attested(&m.chain);

    let mut built_again = None;
    for i in 0..INTERVALS {
        mesh.now += 250;
        mesh.tick();
        mesh.pump(2_000);
        if !mesh.members[2].driver.as_ref().unwrap().is_follower() {
            built_again = Some(i);
            break;
        }
        catch_up_member(&mut mesh, 2, 0, &committee);
    }
    let at = built_again.unwrap_or_else(|| {
        panic!("still a follower after {INTERVALS} intervals: it never builds again")
    });
    println!("built again after {at} interval(s)");

    // From here on nothing is imported: every block it gains it built.
    let (lead, own) = (
        mesh.members[0].chain.height(),
        mesh.members[2].chain.height(),
    );
    let round = mesh.members[2].driver.as_ref().unwrap().round();
    for _ in 0..8 {
        mesh.now += 250;
        mesh.tick();
        mesh.pump(2_000);
    }
    let now_own = mesh.members[2].chain.height();
    assert!(now_own > own + 3, "it builds: {own} -> {now_own}");
    assert!(
        mesh.members[2].driver.as_ref().unwrap().round() > round + 4,
        "it proposes"
    );
    assert!(mesh.members[0].chain.height() > lead, "the network goes on");
    mesh.agree_at(now_own.min(mesh.members[0].chain.height()));
    assert_eq!(mesh.equivocations, 0);
}

/// Holds member `i` out for 60 blocks, then restarts it as an attested
/// follower of member 0's checkpoints. Returns how many blocks it imported.
fn restart_as_follower(mesh: &mut Mesh, i: usize, committee: &[VerifyingKey]) -> u64 {
    mesh.members[i].driver = None;
    let before = mesh.members[0].chain.height();
    mesh.run_to(before + 60);
    let m = &mut mesh.members[i];
    m.chain = open_chain(&m.state_dir.with_extension("scratch"), false);
    m.chain = open_chain(&m.state_dir, false);
    let imported = catch_up_member(mesh, i, 0, committee);
    let m = &mut mesh.members[i];
    let (mut driver, step) =
        BftDriver::open(&mesh.setups[i], &m.bft_dir, &mut m.chain, mesh.now).unwrap();
    driver.follow_attested(&m.chain);
    m.driver = Some(driver);
    mesh.absorb(i, step);
    imported
}

#[test]
fn two_builders_keep_finalizing_while_both_other_validators_follow() {
    // n = 4, f = 1: members 2 and 3 restart far behind, one after the other,
    // and follow. Only 2f builders are left. Followers vote, so the DAG keeps
    // its quorum and the builders keep building. Checkpoints (n - f
    // attesters) stop, so checkpoint imports alone never bring the followers
    // back; in the node that case is ADR-052's certificate replay
    // (`bft_follower_tests`), which this mesh does not run.
    let (_, committee) = signers();
    let mut mesh = Mesh::new(0);
    mesh.run_to(3);
    restart_as_follower(&mut mesh, 2, &committee);
    restart_as_follower(&mut mesh, 3, &committee);
    let start = mesh.members[0].chain.height();
    for _ in 0..20 {
        mesh.now += 250;
        mesh.tick();
        mesh.pump(2_000);
        catch_up_member(&mut mesh, 2, 0, &committee);
        catch_up_member(&mut mesh, 3, 0, &committee);
    }
    let lead = mesh.members[0].chain.height();
    let followers: Vec<bool> = (2..4)
        .map(|i| mesh.members[i].driver.as_ref().unwrap().is_follower())
        .collect();
    println!(
        "two followers: builders {start} -> {lead}; still following {followers:?}; heights {:?}",
        mesh.members
            .iter()
            .map(|m| m.chain.height())
            .collect::<Vec<_>>()
    );
    assert!(lead > start + 5, "two builders stalled: {start} -> {lead}");
    mesh.agree_at(mesh.live().map(|m| m.chain.height()).min().unwrap());
    assert_eq!(mesh.equivocations, 0);
}

/// Imports into member `lagging` from member `from`'s checkpoint and chain.
fn catch_up_member(
    mesh: &mut Mesh,
    lagging: usize,
    from: usize,
    committee: &[VerifyingKey],
) -> u64 {
    let (left, right) = mesh.members.split_at_mut(lagging);
    let peer = Peer {
        chain: &left[from].chain,
        checkpoint: left[from].driver.as_ref().unwrap().checkpoint().cloned(),
        tip: left[from].chain.height(),
    };
    custom_l1_node::consensus::bft::catchup::catch_up(&mut right[0].chain, &peer, committee, None)
        .expect("catch-up from a peer's checkpoint")
}

/// Offers member 3's newest checkpoint to `book`, as the node binary publishes
/// it, and returns the newest height seen.
fn offer_from_three(mesh: &Mesh, book: &mut CheckpointBook, seen: u64) -> u64 {
    match mesh.members[3].driver.as_ref().unwrap().checkpoint() {
        Some(c) => {
            book.offer(c);
            seen.max(c.height)
        }
        None => seen,
    }
}

/// Imports into member 2 from member 3 posing as a follower: its reported tip
/// one block behind the newest checkpoint it has seen (pre-freeze audit M-2).
fn catch_up_from_follower(
    mesh: &mut Mesh,
    book: &mut CheckpointBook,
    seen: u64,
    committee: &[VerifyingKey],
) -> u64 {
    let (left, right) = mesh.members.split_at_mut(3);
    let tip = seen.saturating_sub(1).min(right[0].chain.height());
    book.set_tip(tip);
    let peer = Peer {
        chain: &right[0].chain,
        checkpoint: book.newest().cloned(),
        tip,
    };
    custom_l1_node::consensus::bft::catchup::catch_up(&mut left[2].chain, &peer, committee, None)
        .expect("catch-up from a follower's held checkpoint")
}

/// When validator 2's engine opens on restart, relative to its catch-up.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Engine {
    /// Never: it only imports.
    Closed,
    /// After the first import, a few blocks behind the network.
    AfterImport,
    /// Before any import, 60 blocks behind, the order `maya2c-node` uses.
    BeforeImport,
}

/// Opens validator 2's engine on its safety log, at its chain's tip.
fn open_member_two(mesh: &mut Mesh) {
    let m = &mut mesh.members[2];
    let (driver, step) =
        BftDriver::open(&mesh.setups[2], &m.bft_dir, &mut m.chain, mesh.now).unwrap();
    m.driver = Some(driver);
    mesh.absorb(2, step);
}

/// Validator 2 down far past the engine window, then restarted with member 3,
/// posing as a follower one block behind, as its only catch-up source; with
/// an engine, it follows attested blocks after the import. Returns the
/// heights `(start, lead, rejoined)` over 30 more intervals of following that
/// source and, with an engine, how many rounds it trails member 0's.
fn follower_source_scenario(engine: Engine) -> (u64, u64, u64, Option<u64>) {
    let mut mesh = Mesh::new(0);
    mesh.run_to(3);
    mesh.members[2].driver = None;
    let before = mesh.members[2].chain.height();
    let mut book = CheckpointBook::default();
    let mut seen = 0;
    for _ in 0..400 {
        if mesh.members[0].chain.height() >= before + 60 {
            break;
        }
        seen = step_following(&mut mesh, &mut book, seen);
    }
    assert!(
        mesh.members[0].chain.height() >= before + 60,
        "3 of 4 stalled"
    );

    let m = &mut mesh.members[2];
    m.chain = open_chain(&m.state_dir.with_extension("scratch"), false);
    m.chain = open_chain(&m.state_dir, false);
    let (_, committee) = signers();
    if engine == Engine::BeforeImport {
        // The chain where the outage left it, and the network running on: a
        // live engine this far behind must not hold the builders up.
        open_member_two(&mut mesh);
        let from = mesh.members[0].chain.height();
        let behind = from - mesh.members[2].chain.height();
        assert!(behind >= 50, "only {behind} blocks behind");
        for _ in 0..8 {
            seen = step_following(&mut mesh, &mut book, seen);
        }
        assert!(
            mesh.members[0].chain.height() > from + 2,
            "an engine {behind} blocks behind stalled the builders at {from}"
        );
    }
    let imported = catch_up_from_follower(&mut mesh, &mut book, seen, &committee);
    assert!(imported >= 50, "imported {imported}");
    if engine == Engine::AfterImport {
        open_member_two(&mut mesh);
    }
    if engine != Engine::Closed {
        // What the binary's `catch_up` does once it has imported.
        let m = &mut mesh.members[2];
        m.driver.as_mut().unwrap().follow_attested(&m.chain);
    }

    let start = mesh.members[0].chain.height();
    for _ in 0..30 {
        seen = step_following(&mut mesh, &mut book, seen);
        catch_up_from_follower(&mut mesh, &mut book, seen, &committee);
    }
    let (lead, rejoined) = (
        mesh.members[0].chain.height(),
        mesh.members[2].chain.height(),
    );
    let at = usize::try_from(rejoined).unwrap();
    assert_eq!(
        mesh.members[2].chain.active_chain().unwrap()[at],
        mesh.members[0].chain.active_chain().unwrap()[at],
        "the imported chain is the network's"
    );
    assert_eq!(mesh.equivocations, 0);
    let trails = mesh.members[2].driver.as_ref().map(|d| {
        let lead_round = mesh.members[0].driver.as_ref().unwrap().round();
        lead_round.saturating_sub(d.round())
    });
    (start, lead, rejoined, trails)
}

/// One mesh interval, then member 3's newest checkpoint offered to `book`.
fn step_following(mesh: &mut Mesh, book: &mut CheckpointBook, seen: u64) -> u64 {
    mesh.now += 250;
    mesh.tick();
    mesh.pump(2_000);
    offer_from_three(mesh, book, seen)
}

#[test]
fn a_restarting_validator_catches_up_from_a_follower_source_alone() {
    // M-2 and gate 10: the restarting validator's only `--catch-up-from` source
    // trails the newest checkpoint it has seen. It used to serve that one, which
    // it held no block for, so nothing was ever imported. Now the first import
    // takes it 50+ blocks, and it keeps pace with the network from that source.
    let (start, lead, rejoined, _) = follower_source_scenario(Engine::Closed);
    assert!(
        lead > start + 5,
        "the network kept going: {start} -> {lead}"
    );
    assert!(lead - rejoined <= 4, "it keeps pace: {rejoined} vs {lead}");
}

/// The three builders keep finalizing while a fourth validator's engine runs
/// behind them, and that validator rejoins: it keeps importing, and its
/// engine proposes in the network's rounds. Found 2026-11-08 as a stall at
/// height 105: the stall was this mesh running the engine with the
/// simulator's `resend_interval_ms` of 0 (see `Mesh::new`); the engine left
/// 60 blocks behind then never rejoining was an engine bug (certificates
/// buffered before a resume were never drained). See
/// reports/sessions/2026-11-08-behind-validator-stall.md.
fn assert_rejoined_without_stalling(engine: Engine) {
    let (start, lead, rejoined, trails) = follower_source_scenario(engine);
    assert!(
        lead > start + 5,
        "the network kept going: {start} -> {lead}, rejoined {rejoined}"
    );
    assert!(rejoined > start, "it kept importing: {start} -> {rejoined}");
    let trails = trails.expect("an engine");
    assert!(trails <= 4, "its engine trails by {trails} rounds");
}

#[test]
fn a_validator_opened_behind_the_tip_does_not_stall_the_builders() {
    assert_rejoined_without_stalling(Engine::AfterImport);
}

#[test]
fn a_validator_opened_fifty_blocks_behind_does_not_stall_the_builders() {
    assert_rejoined_without_stalling(Engine::BeforeImport);
}

#[test]
fn a_catch_up_block_already_held_is_not_a_fault() {
    use custom_l1_node::consensus::bft::catchup::{Imported, import};

    let root = TempDir::new().unwrap();
    let mut chain = open_chain(&root.path().join("state"), false);

    // The genesis is already held, so a batch that starts at it overlaps what
    // the node has. That is normal catch-up traffic, not a malformed block.
    let report = import(&mut chain, vec![genesis()]).expect("an overlapping batch imports");

    assert_eq!(
        report,
        Imported {
            applied: 0,
            already_held: 1
        }
    );
}

#[test]
fn a_block_that_is_neither_held_nor_an_extension_still_fails_the_import() {
    use custom_l1_node::consensus::bft::catchup::import;

    let root = TempDir::new().unwrap();
    let mut chain = open_chain(&root.path().join("state"), false);

    // Its parent is one the chain does not hold, so it is not a duplicate and
    // cannot extend the tip. A duplicate must not have loosened this check.
    let mut orphan = genesis();
    orphan.header.prev_hash = [0x55; 32];
    orphan.header.nonce = 1;
    assert!(import(&mut chain, vec![orphan]).is_err());
}
