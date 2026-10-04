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

use custom_l1_node::consensus::bft::{BftDriver, BftSetup, Step};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::keys::{self, SigningKey, VerifyingKey};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::account::Account;
use custom_l1_node::state::db::StateDB;
use maya_dag_bft::Params;
use tempfile::TempDir;

mod common;

const VALIDATORS: usize = 4;
const RECIPIENT: [u8; 32] = [0x77; 32];
const START_BALANCE: u64 = 1_000_000;

fn genesis() -> Block {
    Block::new(
        BlockHeader {
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
            amount,
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
                    balance: START_BALANCE,
                    nonce: 0,
                },
            )
            .unwrap();
    }
    common::open_chain(state, genesis(), ChainConfig::without_pow_verification()).unwrap()
}

impl Mesh {
    /// Four validators and `observers` keyless followers.
    fn new(observers: usize) -> Self {
        let root = TempDir::new().unwrap();
        let (keys, committee) = signers();
        let params = Params {
            batch_size: 100,
            anchor_timeout_ms: 1_000,
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
        };
        for (i, step) in opening {
            mesh.absorb(i, step);
        }
        mesh
    }

    fn absorb(&mut self, from: usize, step: Step) {
        self.equivocations += step.equivocations.len();
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
        for _ in 0..400 {
            if self.live().all(|m| m.chain.height() >= height) {
                return;
            }
            self.now += 250;
            self.tick();
            self.pump(2_000);
        }
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
            .verify(&tag, &committee)
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
}

impl custom_l1_node::consensus::bft::catchup::CheckpointSource for Peer<'_> {
    fn checkpoint(
        &self,
    ) -> custom_l1_node::error::Result<Option<custom_l1_node::consensus::bft::attest::Checkpoint>>
    {
        Ok(self.checkpoint.clone())
    }

    fn block(&self, height: u64) -> custom_l1_node::error::Result<Block> {
        let state = self.chain.state();
        let id = state.canonical_id(height)?.expect("peer holds the height");
        Ok(state.load_block(&id)?.expect("peer holds the body"))
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
    };
    custom_l1_node::consensus::bft::catchup::catch_up(&mut right[0].chain, &peer, committee)
        .expect("catch-up from a peer's checkpoint")
}
