//! Chaos simulation: what the chain does while the network comes apart.
//!
//! # Verified, or merely characterised
//!
//! Every scenario below is one of two things, and the name says which:
//!
//! - **verified** — the chain has a defence and this checks it holds;
//! - **characterisation** — the chain has *no* defence, and this records what
//!   happens instead so nobody has to discover it on a testnet.
//!
//! The distinction is the whole value of the file. A green tick next to
//! "out-of-order arrival" that implied buffering the node does not do would be
//! worse than no test at all. `attack_simulation_tests.rs` already sets this
//! precedent with its sybil and peer-scoring characterisations.
//!
//! # What the brief asked for, and what this chain actually is
//!
//! The brief was written against a different architecture, and mapping it onto
//! this one changed three things. They are recorded here because the mapping is
//! the interesting part:
//!
//! | Asked for | This chain | Tested instead |
//! |---|---|---|
//! | 33% Byzantine *validator* crashes | Proof of work. No validator set, no stake, no quorum, no permissioning | 2 of 6 nodes crash, **and** 2 of 6 turn actively malicious |
//! | out-of-order *DAG vertex* arrival | `crypto::dag` is the Ethash-style PoW *dataset* — it has no vertices. `blockgraph` has them but nothing in consensus calls it | out-of-order **block** arrival |
//! | finality within 3 *DAG rounds* | No rounds and no finality; PoW settles probabilistically | reconvergence within **3 blocks** of the partition healing |
//!
//! # What is deliberately not here
//!
//! Double-spends and conflicting spends in one block are in
//! `attack_simulation_tests.rs`. Invalid ML-DSA and SLH-DSA signatures — every
//! bit of both halves flipped — are in `malleability_tests.rs`. Fork choice and
//! reorg atomicity are in `consensus_tests.rs`. Decoder robustness against
//! arbitrary bytes is in `fuzz/fuzz_targets/*_decode.rs`.
//!
//! Re-implementing any of that here would produce a second copy to keep in
//! step. The corruption engine below deliberately targets the layer *above* the
//! fuzz targets: input that decodes cleanly and is then semantically hostile,
//! driven through `Chain::insert_block` rather than through `from_bytes`.
//!
//! # Determinism
//!
//! There is no `rand` in `[dev-dependencies]`, and a chaos test whose failures
//! cannot be replayed is not worth running. Everything random here comes from
//! an inline SplitMix64 seeded from [`DEFAULT_SEED`], overridable with
//! `MAYA_CHAOS_SEED`, and the seed is printed and written into the report.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use custom_l1_node::consensus::BlockId;
use custom_l1_node::consensus::chain::{Chain, ChainConfig, InsertOutcome};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::network::{LatencyDial, Mempool, Node, NodeHandle};
use custom_l1_node::state::BlockContext;
use custom_l1_node::state::account::{Account, Address};
use custom_l1_node::state::db::StateDB;
use libp2p::Multiaddr;
use tempfile::TempDir;

mod report;

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

/// Seed used when `MAYA_CHAOS_SEED` is unset.
pub const DEFAULT_SEED: u64 = 0x4D41_5941_3243_0001;

/// The seed this run is using.
///
/// Printed by every test that consumes randomness, so a CI failure can be
/// replayed with `MAYA_CHAOS_SEED=<n> cargo test --test chaos_simulator`.
pub fn seed() -> u64 {
    std::env::var("MAYA_CHAOS_SEED")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(DEFAULT_SEED)
}

/// The randomness for every scenario below comes from `maya-sim`.
///
/// It used to be a private nine-line SplitMix64 in this file, with the
/// argument that adding a dependency so a test could shuffle a vector was a
/// poor trade. That was true while it was the only one. It is not true now:
/// `sim/` is the workspace's deterministic harness, every later chaos,
/// latency, partition and Byzantine test is meant to run on it, and a second
/// private generator here would be a second definition of "the seed" —
/// exactly the thing that makes a recorded seed stop meaning anything.
///
/// `SimRng` is the same algorithm, so the scenarios below are unchanged.
/// What is new is `maya_sim::replay`, which prints the seed on failure, and
/// `below`, which is uniform without a modulo bias.
///
/// See `docs/adr/ADR-006-simulation-harness.md`.
use maya_sim::SimRng;

/// A value in `0..bound`, as a `usize`, which is what this file wants
/// everywhere.
fn below(rng: &mut SimRng, bound: usize) -> usize {
    assert!(bound > 0, "bound must be positive");
    rng.below(bound as u64) as usize
}

// ---------------------------------------------------------------------------
// Chain harness
// ---------------------------------------------------------------------------
//
// Deliberately the same shape as `attack_simulation_tests.rs:41-99`. Sharing it
// through a `mod common` would couple two files whose reasons to change are
// different — this one exists to be edited as new failure modes are found.

fn open_state() -> (Arc<StateDB>, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open state");
    (Arc::new(db), dir)
}

fn genesis() -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

/// A chain with proof-of-work verification off.
///
/// Mining a real nonce per block would make this file take minutes and would
/// test the hasher, which `dag_tests.rs` already does. What is under test here
/// is the state transition engine's behaviour on adversarial input.
fn test_chain(state: Arc<StateDB>) -> Chain {
    Chain::open(state, genesis(), ChainConfig::without_pow_verification()).expect("open chain")
}

fn child_of(
    chain: &Chain,
    parent: BlockId,
    timestamp: u64,
    transactions: Vec<Transaction>,
) -> Block {
    let target = chain.next_target(&parent).expect("next target");
    let mut block = Block::new(
        BlockHeader {
            prev_hash: parent,
            state_root: [0u8; 32],
            timestamp,
            nonce: 0,
            difficulty_target: target,
            tx_root: [0; 32],
        },
        transactions,
    );
    // Declare the root the block executes to, as a miner would: the chain
    // refuses any other. Only computable on the tip, which is where every
    // caller builds. A block that cannot execute keeps the zero root; the
    // chain refuses it for its transactions before any root is compared.
    if parent == chain.tip()
        && let Ok(root) = chain
            .state()
            .preview_root(&block, BlockContext::at_height(chain.height() + 1))
    {
        block.header.state_root = root;
    }
    block
}

fn address_of(key: &HybridSigningKey) -> Address {
    key.address()
}

fn transfer(from: &HybridSigningKey, to: Address, amount: u64, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount,
            recipient: to,
        }],
        nonce,
    );
    tx.sign(from).expect("sign");
    tx
}

/// A funded chain and the key that owns the balance.
fn funded_chain(balance: u64) -> (Chain, HybridSigningKey, TempDir) {
    let key = generate_signing_key().expect("keygen");
    let (chain, dir) = chain_funding(&key, balance);
    (chain, key, dir)
}

/// A chain whose genesis state funds `key`.
///
/// Split out from [`funded_chain`] because two chains modelling two nodes on
/// one network must start from the *same* state. Generating a key per chain
/// gives them different genesis states, and they then disagree about the state
/// root for reasons that have nothing to do with what is under test — which is
/// exactly the false failure the partition test's state-root assertion caught.
fn chain_funding(key: &HybridSigningKey, balance: u64) -> (Chain, TempDir) {
    let (state, dir) = open_state();
    state
        .put_account(&address_of(key), &Account { balance, nonce: 0 })
        .expect("fund");
    (test_chain(Arc::clone(&state)), dir)
}

/// Builds `count` valid blocks extending `chain`, inserting each as it goes.
///
/// Insertion is not incidental: `child_of` asks the chain for the next target,
/// and the chain can only answer for a parent it already holds. Building a
/// branch without inserting it is therefore not possible, which is why the
/// out-of-order tests mint their blocks on a scratch chain and deliver them to
/// a second one.
fn build_branch(
    chain: &mut Chain,
    from: BlockId,
    count: usize,
    first_timestamp: u64,
) -> Vec<Block> {
    let mut blocks = Vec::with_capacity(count);
    let mut parent = from;
    for step in 0..count {
        let block = child_of(
            chain,
            parent,
            first_timestamp + step as u64 * 10,
            Vec::new(),
        );
        parent = block.header.id();
        chain.insert_block(block.clone()).expect("minting extends");
        blocks.push(block);
    }
    blocks
}

/// A branch minted on a throwaway chain, for delivering to a different one.
///
/// The scratch chain shares this file's deterministic [`genesis`] *and* its
/// genesis state: `key` funded with `balance`, exactly as the receiving chain
/// is. Each block declares the state root it executes to, so a scratch chain
/// funding some other key would mint blocks whose roots no receiving chain
/// reproduces — the same trap [`chain_funding`] describes.
fn mint_branch(
    key: &HybridSigningKey,
    balance: u64,
    count: usize,
    first_timestamp: u64,
) -> Vec<Block> {
    let (mut scratch, _dir) = chain_funding(key, balance);
    let from = scratch.tip();
    build_branch(&mut scratch, from, count, first_timestamp)
}

/// The chain's tip and state root together.
///
/// The pair is the thing "state integrity" means: a tip that did not move while
/// the state root did is corruption that a tip-only assertion would miss.
fn fingerprint(chain: &Chain) -> (BlockId, [u8; 32]) {
    (chain.tip(), chain.state().state_root().expect("state root"))
}

// ---------------------------------------------------------------------------
// Out-of-order block arrival
// ---------------------------------------------------------------------------

#[test]
fn characterisation_a_block_whose_parent_has_not_arrived_is_dropped_not_buffered() {
    // **There is no orphan pool anywhere in this node.**
    //
    // `Chain::insert_block` calls `require(&parent_id)?` and errors; nothing in
    // `network/node.rs` holds the block for later. So a block that arrives
    // early is not deferred — it is gone, and the sender must re-send it.
    //
    // That is a real property of a testnet under reordering, and it is recorded
    // rather than asserted-away: gossip does not guarantee order, so a node
    // that misses a parent stalls until some other path re-delivers the child.
    //
    // If an orphan pool is ever added, this test fails and should be rewritten
    // as a verification. That is the intended signal.
    let (mut chain, key, _dir) = funded_chain(1_000_000);
    let branch = mint_branch(&key, 1_000_000, 8, 1_000_100);

    let before = fingerprint(&chain);

    // Deliver everything except the first block. Every one of them has an
    // unknown parent.
    for block in branch.iter().skip(1) {
        let outcome = chain.insert_block(block.clone());
        assert!(
            outcome.is_err(),
            "a block whose parent is unknown must be refused, not buffered"
        );
    }

    assert_eq!(
        fingerprint(&chain),
        before,
        "eight refused blocks must leave the tip and the state root untouched"
    );

    // In order, the same blocks are accepted. The chain is not poisoned by the
    // rejections; the ordering is the only thing that was wrong.
    for block in &branch {
        chain
            .insert_block(block.clone())
            .expect("in-order delivery");
    }
    assert_eq!(chain.tip(), branch.last().expect("branch").header.id());
    assert_eq!(chain.height(), 8);

    report::record(
        "out-of-order block arrival",
        report::Finding::Characterised,
        "there is no orphan pool: a block whose parent has not arrived is refused and discarded, not buffered. 7 early blocks left tip and state root untouched; in-order re-delivery of the same blocks then reached height 8",
    );
}

#[test]
fn characterisation_a_shuffled_branch_lands_only_as_far_as_its_prefix() {
    // The same property under a seeded shuffle rather than a hand-picked order,
    // so the recorded behaviour is not an artefact of one arrangement.
    //
    // What a caller can rely on: whatever arrives, the chain ends on a tip that
    // is a real block it accepted, and never on a partial or invented one.
    let run_seed = seed();
    println!("MAYA_CHAOS_SEED={run_seed}");
    let mut rng = SimRng::new(run_seed);

    let (mut chain, key, _dir) = funded_chain(1_000_000);
    let branch = mint_branch(&key, 1_000_000, 8, 1_000_100);

    let mut shuffled: Vec<Block> = branch.clone();
    rng.shuffle(&mut shuffled);

    let mut accepted = 0usize;
    for block in &shuffled {
        // The assertion that matters is the absence of a panic here: an
        // out-of-order block must produce a `Result`, never unwind.
        if chain.insert_block(block.clone()).is_ok() {
            accepted += 1;
        }
    }

    assert!(
        chain.contains(&chain.tip()),
        "the tip must always be a block the chain actually holds"
    );
    assert_eq!(
        chain.height() as usize,
        accepted,
        "height must equal the number of blocks accepted, with no gaps invented"
    );

    // Re-delivering in order completes the branch, whatever the shuffle did.
    for block in &branch {
        let _ = chain.insert_block(block.clone());
    }
    assert_eq!(chain.tip(), branch.last().expect("branch").header.id());

    report::record(
        "shuffled branch delivery",
        report::Finding::Characterised,
        &format!(
            "a seeded shuffle of an 8-block branch landed {accepted} blocks with no panic; height always equalled the number accepted, and the tip was always a block the chain held. In-order re-delivery completed the branch"
        ),
    );
}

// ---------------------------------------------------------------------------
// Corruption engine
// ---------------------------------------------------------------------------

/// One way to damage an encoded structure.
///
/// Each variant is a distinct *class* of malformation rather than a random
/// smear, so a failure names the shape of the input that caused it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corruption {
    /// Flip one bit.
    FlipBit,
    /// Cut the tail off.
    Truncate,
    /// Append junk.
    Extend,
    /// Zero a run of bytes.
    ZeroRun,
    /// Swap two byte ranges, so length is preserved and structure is not.
    SwapRanges,
}

/// Every corruption class, for a caller that wants to sweep them.
pub const CORRUPTIONS: [Corruption; 5] = [
    Corruption::FlipBit,
    Corruption::Truncate,
    Corruption::Extend,
    Corruption::ZeroRun,
    Corruption::SwapRanges,
];

impl Corruption {
    /// Applies this corruption, returning `None` if the input is too small for
    /// it to be meaningful.
    ///
    /// `None` rather than a silent no-op: a corruption engine that quietly
    /// returns its input unchanged passes every assertion while testing
    /// nothing, which is the failure mode this whole file exists to avoid.
    #[must_use]
    pub fn apply(self, bytes: &[u8], rng: &mut SimRng) -> Option<Vec<u8>> {
        if bytes.is_empty() {
            return None;
        }
        let mut out = bytes.to_vec();

        match self {
            Self::FlipBit => {
                let index = below(rng, out.len());
                out[index] ^= 1 << (below(rng, 8) as u8);
            }
            Self::Truncate => {
                if out.len() < 2 {
                    return None;
                }
                let keep = below(rng, out.len() - 1);
                out.truncate(keep);
            }
            Self::Extend => {
                let count = 1 + below(rng, 32);
                for _ in 0..count {
                    out.push(below(rng, 256) as u8);
                }
            }
            Self::ZeroRun => {
                let start = below(rng, out.len());
                let end = (start + 1 + below(rng, 16)).min(out.len());
                // A run that is already zero changes nothing, which would be
                // the silent no-op named above.
                if out[start..end].iter().all(|b| *b == 0) {
                    return None;
                }
                out[start..end].fill(0);
            }
            Self::SwapRanges => {
                if out.len() < 4 {
                    return None;
                }
                let width = 1 + below(rng, out.len() / 2);
                let a = below(rng, out.len() - width);
                let b = below(rng, out.len() - width);
                if a == b {
                    return None;
                }
                for offset in 0..width {
                    out.swap(a + offset, b + offset);
                }
            }
        }

        // The engine must actually change something. Returning `None` here
        // rather than the unchanged bytes is what keeps a vacuous pass
        // impossible.
        if out == bytes { None } else { Some(out) }
    }
}

#[test]
fn the_corruption_engine_actually_corrupts() {
    // The engine's own test. Without it, a bug that made every mutation a
    // no-op would turn every test below into a test of nothing that still
    // passes green.
    let mut rng = SimRng::new(seed());
    let subject: Vec<u8> = (0..=255u8).collect();

    for corruption in CORRUPTIONS {
        let mut produced = 0;
        for _ in 0..64 {
            if let Some(damaged) = corruption.apply(&subject, &mut rng) {
                assert_ne!(damaged, subject, "{corruption:?} returned its input");
                produced += 1;
            }
        }
        assert!(
            produced > 0,
            "{corruption:?} never produced a corrupted output in 64 attempts"
        );
    }
}

#[test]
fn a_corrupted_block_is_rejected_without_moving_the_state() {
    // The "absolute state integrity" clause, at the layer the fuzz targets do
    // not reach: these bytes are fed to `Chain::insert_block`, so anything that
    // decodes goes on to be validated and executed against the real `StateDB`.
    //
    // Two assertions, and the second is the load-bearing one:
    //   1. no corrupted block is accepted, and nothing panics;
    //   2. the tip *and* the state root are byte-identical afterwards.
    //
    // A tip-only assertion would miss a partially-applied block: state written
    // before a mid-execution failure, with the tip correctly left behind.
    let run_seed = seed();
    println!("MAYA_CHAOS_SEED={run_seed}");
    let mut rng = SimRng::new(run_seed);

    let (mut chain, key, _dir) = funded_chain(1_000_000);
    let recipient = [0x22u8; 32];

    // A real block with a real signed transaction, so what gets corrupted is
    // the shape the chain actually handles.
    let honest = child_of(
        &chain,
        chain.tip(),
        1_000_100,
        vec![transfer(&key, recipient, 1_000, 0)],
    );
    chain
        .insert_block(honest.clone())
        .expect("the honest block is accepted");

    let before = fingerprint(&chain);
    let encoded = honest.to_bytes();

    let mut attempted = 0usize;
    let mut decoded = 0usize;
    let mut rejected = 0usize;
    let mut duplicates = 0usize;

    for corruption in CORRUPTIONS {
        for _ in 0..40 {
            let Some(damaged) = corruption.apply(&encoded, &mut rng) else {
                continue;
            };
            attempted += 1;

            // A corrupted encoding either fails to decode -- which the fuzz
            // targets already cover -- or decodes into something the chain must
            // refuse. Only the second case is this test's business.
            let Ok(block) = Block::from_bytes(&damaged) else {
                continue;
            };
            decoded += 1;

            match chain.insert_block(block) {
                Err(_) => rejected += 1,
                // Before the header committed to its transactions, every
                // corruption that landed in a transaction came back here: the
                // id is the header hash, so the chain took the damaged block
                // for the one it held. `check_tx_root` now runs ahead of the
                // duplicate check, so only a corruption that decodes back to
                // byte-identical transactions can still reach this arm, and
                // that really is the same block.
                Ok(InsertOutcome::Duplicate { .. }) => {
                    duplicates += 1;
                }
                Ok(other) => panic!("a corrupted block was accepted as {other:?}"),
            }
        }
    }

    assert!(
        attempted > 100,
        "only {attempted} corruptions were produced"
    );
    assert!(
        decoded > 0,
        "no corrupted block ever decoded, so the state transition engine was \
         never actually exercised -- this test would be vacuous"
    );

    assert_eq!(
        fingerprint(&chain),
        before,
        "{rejected} rejected and {duplicates} indistinguishable blocks must leave tip \
         and state root identical"
    );
    // Every decode is accounted for. Without this the two counters could drift
    // apart and the report would quote numbers that do not add up.
    assert_eq!(
        decoded,
        rejected + duplicates,
        "every decoded block must be either rejected or found indistinguishable"
    );

    // Characterised or verified is decided by the run, not asserted in advance.
    //
    // The state integrity above *is* a real defence and is genuinely verified.
    // But if nothing was rejected, no defence fired, and labelling that
    // "verified" would credit the chain with a defence it did not exercise —
    // the single thing this report exists to avoid. Before `tx_root` that was
    // exactly what happened: every decoded corruption was a `Duplicate`.
    let finding = if rejected > 0 {
        report::Finding::Verified
    } else {
        report::Finding::Characterised
    };
    report::record(
        "corrupted blocks",
        finding,
        &format!(
            "{attempted} corruptions across {} classes; {decoded} decoded, of which {rejected} \
             were rejected and {duplicates} decoded back to the identical block. Tip and state \
             root unchanged",
            CORRUPTIONS.len()
        ),
    );
}

#[test]
fn a_corrupted_transaction_is_rejected_without_moving_the_state() {
    // The same discipline one level down. A transaction that decodes but whose
    // signature, nonce or amount has been tampered with must fail its block,
    // and a failing transaction fails its *whole* block here -- so the state
    // must be exactly as it was.
    let run_seed = seed();
    println!("MAYA_CHAOS_SEED={run_seed}");
    let mut rng = SimRng::new(run_seed);

    let (mut chain, key, _dir) = funded_chain(1_000_000);
    let recipient = [0x33u8; 32];
    let honest = transfer(&key, recipient, 500, 0);
    let encoded = honest.to_bytes();

    let before = fingerprint(&chain);
    let mut exercised = 0usize;

    for corruption in CORRUPTIONS {
        for _ in 0..40 {
            let Some(damaged) = corruption.apply(&encoded, &mut rng) else {
                continue;
            };
            let Ok(tx) = Transaction::from_bytes(&damaged) else {
                continue;
            };
            // A corruption can land on a byte the codec ignores, reproducing
            // the honest transaction. That is not a tampered transaction and
            // proves nothing either way.
            if tx == honest {
                continue;
            }
            exercised += 1;

            let block = child_of(&chain, chain.tip(), 1_000_100, vec![tx]);
            assert!(
                chain.insert_block(block).is_err(),
                "a block carrying a tampered transaction must be refused"
            );
        }
    }

    assert!(
        exercised > 0,
        "no tampered transaction ever decoded -- this test would be vacuous"
    );
    assert_eq!(
        fingerprint(&chain),
        before,
        "{exercised} refused blocks must leave tip and state root identical"
    );

    report::record(
        "corrupted transactions",
        report::Finding::Verified,
        &format!(
            "{exercised} tampered transactions decoded and were carried in a block; every block was refused and tip and state root were unchanged"
        ),
    );
}

// ---------------------------------------------------------------------------
// Closed finding: a block's transaction list and state root are committed
// ---------------------------------------------------------------------------

#[test]
fn a_blocks_transaction_list_and_state_root_are_committed() {
    // This was a consensus vulnerability, found by the corruption engine above:
    // every corrupted block came back `Duplicate`, which should have been
    // impossible. The header had no field for the transactions, so `Block::id()`
    // and the proof of work covered the header alone. Anyone could swap an
    // honest block's transactions for their own, keep its id, and ride the
    // work its miner paid for. And `apply_block_journaled` never compared the
    // declared state root with the one execution produced.
    //
    // The fix is `BlockHeader::tx_root`, which `Chain::insert_block` checks
    // before anything else, and a checked `apply_block_journaled`. This test
    // replays the original attack and each way around the fix.
    let key = generate_signing_key().expect("keygen");
    let fresh_node = || chain_funding(&key, 1_000_000);

    let (honest_miner, _dir) = fresh_node();
    let honest = honest_miner
        .candidate_block(1_000_100, vec![transfer(&key, [0x99u8; 32], 100, 0)])
        .expect("candidate");

    // 1. The original attack: the honest header, someone else's transactions,
    //    delivered to a node that has not seen the original.
    let substituted = Block {
        header: honest.header.clone(),
        transactions: vec![transfer(&key, [0x99u8; 32], 900, 0)],
    };
    let (mut victim, _victim_dir) = fresh_node();
    let before = fingerprint(&victim);
    assert!(
        matches!(
            victim.insert_block(substituted.clone()),
            Err(NodeError::TxRootMismatch { .. })
        ),
        "a substituted body must be refused"
    );
    assert_eq!(
        fingerprint(&victim),
        before,
        "a refused body moved the chain"
    );

    // 2. No censorship by poisoning: the refused body was not stored under the
    //    honest id, so the genuine block still lands rather than bouncing off
    //    as a `Duplicate`.
    assert!(
        matches!(
            victim.insert_block(honest.clone()),
            Ok(InsertOutcome::Extended { .. })
        ),
        "the genuine block must still be accepted after a substitution attempt"
    );

    // 3. Recomputing the root to match does not help: it changes the header,
    //    so the attacker has a new block with a new id and no proof of work.
    let resealed = Block::new(honest.header.clone(), substituted.transactions);
    assert_ne!(resealed.header.id(), honest.header.id());

    // 4. The state root is checked. The right transactions under a declared
    //    root execution does not produce are refused, and nothing is written.
    let (mut other, _other_dir) = fresh_node();
    let mut lying = honest.clone();
    lying.header.state_root = [0xEE; 32];
    let before = fingerprint(&other);
    assert!(
        matches!(
            other.insert_block(lying),
            Err(NodeError::StateRootMismatch { .. })
        ),
        "a block must execute to the state root it declares"
    );
    assert_eq!(
        fingerprint(&other),
        before,
        "a refused block moved the state"
    );

    report::record(
        "block transaction list is committed",
        report::Finding::Verified,
        "A substituted body under an honest header is refused (`TxRootMismatch`) before anything is stored, the genuine block still lands afterwards, a recomputed `tx_root` changes the block id, and a false `state_root` is refused (`StateRootMismatch`) with tip and state unchanged",
    );
}

// ---------------------------------------------------------------------------
// Tampered lattice proof-of-useful-work
// ---------------------------------------------------------------------------

#[test]
fn research_branch_a_tampered_lattice_solution_is_rejected_without_panicking() {
    // **This is not a live attack surface.** `lattice-pow` is a research
    // branch: its activation height is `u64::MAX`, nothing in `src/consensus/`
    // calls it, and no block on any chain carries one of these proofs. The
    // brief asked for tampered PoUW solutions, so the verifier is exercised
    // directly and labelled for what it is.
    //
    // What is genuinely worth checking is that the verifier returns `Err` on
    // hostile input rather than panicking, because it does integer arithmetic
    // on attacker-supplied `i64`s -- `verify.rs` already pins the
    // most-negative-coordinate case for exactly that reason.
    use maya_lattice_pow::basis::Basis;
    use maya_lattice_pow::params::LatticeParams;
    use maya_lattice_pow::verify::verify_solution;

    let run_seed = seed();
    println!("MAYA_CHAOS_SEED={run_seed}");
    let mut rng = SimRng::new(run_seed);

    // Small parameters: dimension 8 over q = 97. There is no `TESTING`
    // constant -- the params arrive from the chain by design, so the crate
    // ships none.
    let params = LatticeParams::new(8, 97).expect("valid params");
    let coefficients: Vec<u32> = (0..params.coefficient_count())
        .map(|index| (index as u32 * 7 + 3) % params.modulus)
        .collect();
    let basis = Basis::new(params, coefficients).expect("a well-formed basis");

    let threshold = u128::from(params.modulus) * u128::from(params.modulus);

    for _ in 0..500 {
        // Deliberately hostile coordinates: extremes as well as small values,
        // because a norm computed by squaring is where an overflow lives.
        let vector: Vec<i64> = (0..params.dimension as usize)
            .map(|_| match below(&mut rng, 6) {
                0 => i64::MIN,
                1 => i64::MAX,
                2 => -(rng.next_u64() as i64),
                3 => rng.next_u64() as i64,
                _ => (below(&mut rng, 2 * params.modulus as usize) as i64) - params.modulus as i64,
            })
            .collect();

        // The assertion is the absence of a panic. A random vector is
        // overwhelmingly not a solution, and either answer is legitimate --
        // what must never happen is an unwind on attacker-controlled input.
        let _ = verify_solution(&basis, &vector, threshold);
    }

    // A vector of the wrong length is a shape error, not a norm to compute.
    let too_short = vec![1i64; params.dimension as usize - 1];
    assert!(verify_solution(&basis, &too_short, threshold).is_err());

    let too_long = vec![1i64; params.dimension as usize + 1];
    assert!(verify_solution(&basis, &too_long, threshold).is_err());

    report::record(
        "tampered lattice PoUW solutions",
        report::Finding::Characterised,
        "500 hostile vectors including i64::MIN and i64::MAX returned a Result without panicking, and wrong-length vectors were refused. Research branch only: lattice-pow has an activation height of u64::MAX and nothing in consensus calls it",
    );
}

// ---------------------------------------------------------------------------
// Partition and reconvergence -- fork choice
// ---------------------------------------------------------------------------
//
// **This is deliberately at the chain layer, not the node layer.**
//
// `src/network/node.rs` decodes a gossiped block, emits `BlockReceived`, and
// stops. It holds no `Chain`, keeps no tip, and runs no fork choice -- block
// application is the embedding binary's job. So "both partitions agree on one
// tip" is not a question the node layer can answer, and asserting it there
// would be asserting something about a component that does not exist.
//
// The gossip half of a partition -- delivery stopping and resuming -- is tested
// separately below, against real nodes.

#[test]
fn a_partition_reconverges_within_three_blocks_of_healing() {
    // Two chains over separate state, fed disjoint branches: that is the
    // partition. Cross-feeding every block both ways is the heal.
    //
    // The bound is 3 blocks, which is the brief's figure. Reconvergence here is
    // a single fork-choice comparison, so the real cost is delivery, not
    // consensus rounds -- the bound is generous rather than tight, and the
    // assertion is that it is not exceeded.
    // One key, funding both sides identically. Two nodes on one network share a
    // genesis state; funding them from different keys would make them two
    // different networks that happen to share a genesis *block*.
    let key = generate_signing_key().expect("keygen");
    let (mut left, _left_dir) = chain_funding(&key, 1_000_000);
    let (mut right, _right_dir) = chain_funding(&key, 1_000_000);

    // Both sides start from the same genesis, which is what makes them one
    // network rather than two chains.
    assert_eq!(left.genesis(), right.genesis());
    let split_point = left.tip();

    // --- partitioned: each side builds without seeing the other -------------
    //
    // Different timestamps so the branches are genuinely distinct blocks. The
    // right branch is longer, so it carries more work and must win.
    let left_branch = build_branch(&mut left, split_point, 2, 1_000_100);
    let right_branch = build_branch(&mut right, split_point, 3, 1_000_500);

    assert_ne!(
        left.tip(),
        right.tip(),
        "the partition must actually have produced a disagreement"
    );
    let left_before = left.tip();

    // --- healed: every block crosses, in order ------------------------------
    let mut left_applied = 0usize;
    for block in &right_branch {
        left.insert_block(block.clone())
            .expect("cross-feed to left");
        left_applied += 1;
    }
    for block in &left_branch {
        right
            .insert_block(block.clone())
            .expect("cross-feed to right");
    }

    // --- converged ----------------------------------------------------------
    assert_eq!(
        left.tip(),
        right.tip(),
        "both sides must agree on one tip once each has seen the other's blocks"
    );
    assert_eq!(
        left.tip(),
        right_branch.last().expect("right branch").header.id(),
        "the heavier branch must win, not the incumbent"
    );
    assert_ne!(
        left.tip(),
        left_before,
        "the lighter side must have reorged"
    );

    assert!(
        left_applied <= 3,
        "reconvergence took {left_applied} blocks, over the 3-block budget"
    );

    // Both sides agree on the *state*, not merely on the tip. A tip-only
    // assertion would pass on two chains that had executed different
    // transactions into different balances.
    assert_eq!(
        left.state().state_root().expect("left root"),
        right.state().state_root().expect("right root"),
        "agreeing on a tip while disagreeing on state is the worse failure"
    );

    report::record(
        "partition and heal (fork choice)",
        report::Finding::Verified,
        &format!(
            "2-block and 3-block branches diverge; the heavier wins after {left_applied} \
             blocks cross; state roots match"
        ),
    );
}

// ---------------------------------------------------------------------------
// Network-level scenarios
// ---------------------------------------------------------------------------

/// Nodes in a simulated network.
///
/// Six, so that 2 is exactly a third and a partition splits 3/3.
const NETWORK_SIZE: usize = 6;

/// Nodes that crash or turn hostile: 33% of [`NETWORK_SIZE`].
const FAULTY: usize = 2;

/// How long any propagation assertion waits before failing.
const PROPAGATION_TIMEOUT: Duration = Duration::from_secs(30);

/// How often a propagation condition is re-checked.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The latency a spike dials in. The brief's figure.
const SPIKE: Duration = Duration::from_millis(5_000);

struct SimNode {
    handle: NodeHandle,
    address: Multiaddr,
    _dir: TempDir,
}

/// A fresh in-process address, so concurrent tests never collide.
fn next_memory_address() -> Multiaddr {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    // Offset by the process id so `cargo nextest`, which runs each test in its
    // own process, cannot have two processes pick the same port.
    let port = NEXT.fetch_add(1, Ordering::Relaxed) + u64::from(std::process::id()) * 10_000;
    format!("/memory/{port}").parse().expect("valid multiaddr")
}

/// Spawns a node funding `accounts`, on a transport reading `dial`.
async fn spawn_sim_node(accounts: &[(Address, u64)], dial: Option<LatencyDial>) -> SimNode {
    let dir = TempDir::new().expect("temp dir");
    let state = StateDB::open(dir.path()).expect("open state");
    for (address, balance) in accounts {
        state
            .put_account(
                address,
                &Account {
                    balance: *balance,
                    nonce: 0,
                },
            )
            .expect("fund");
    }

    let state = Arc::new(state);
    let mut node = match dial {
        Some(dial) => Node::new_memory_with_dial(state, dial).expect("build node"),
        None => Node::new_memory(state).expect("build node"),
    };

    let address = next_memory_address();
    node.listen_on(address.clone()).expect("listen");

    SimNode {
        handle: node.spawn(),
        address,
        _dir: dir,
    }
}

/// Polls until `condition` holds, returning whether it did before the deadline.
///
/// Returns rather than panicking, because several scenarios below assert that
/// something *fails* to propagate — and a helper that panicked on timeout could
/// not express that.
async fn settles(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Publishes repeatedly until every observer holds `tx`, or the deadline passes.
///
/// One publish is not enough across a freshly healed link: gossipsub drops a
/// message published into a topic mesh that has not formed yet, and returns
/// `Ok` for it. Re-announcing while waiting is both what makes this reliable
/// and what a real node does when a peer reappears.
async fn publish_until_seen(
    publisher: &NodeHandle,
    tx: &Transaction,
    observers: &[Mempool],
    timeout: Duration,
) -> bool {
    let txid = tx.txid();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let _ = publisher.publish_transaction(tx).await;
        if observers.iter().all(|pool| pool.contains(&txid)) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Waits until `handle` sees at least `min` peers.
async fn wait_for_peers(handle: &NodeHandle, min: usize) {
    let deadline = tokio::time::Instant::now() + PROPAGATION_TIMEOUT;
    loop {
        let count = handle
            .connected_peers()
            .await
            .map(|peers| peers.len())
            .unwrap_or(0);
        if count >= min {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {min} peers; have {count}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Publishes with retry, returning the last error if it never succeeded.
///
/// Gossipsub refuses a publish with `InsufficientPeers` until the topic mesh has
/// formed, which is a startup race rather than a failure.
///
/// The error is carried out rather than collapsed into a `bool`: a test that
/// fails with "could not publish" after thirty seconds and no reason is a test
/// that costs an hour to diagnose. This one names the cause.
async fn publish_until_accepted(node: &NodeHandle, tx: &Transaction) -> Result<(), String> {
    // Seed the publisher's own pool first.
    //
    // Gossipsub does not loop a message back to its sender, so a publishing
    // node never receives its own transaction and its mempool stays empty. An
    // assertion of the form "every node holds it" is then unsatisfiable by
    // construction -- which is exactly how the first version of these tests
    // failed, for thirty seconds, with a message blaming propagation.
    //
    // `network_tests.rs:234` does the same insert for the same reason, and it
    // is what a real node does: a node that publishes a transaction holds it.
    node.mempool()
        .insert(tx.clone())
        .map_err(|e| format!("local insert: {e}"))?;

    let deadline = tokio::time::Instant::now() + PROPAGATION_TIMEOUT;
    let mut last;
    loop {
        match node.publish_transaction(tx).await {
            Ok(_) => return Ok(()),
            Err(error) => last = error.to_string(),
        }

        if tokio::time::Instant::now() >= deadline {
            return Err(last);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Dials every pair once and waits for a **complete** mesh.
///
/// # Why it waits for every peer, not just one
///
/// The first version of this waited for one peer per node, which is enough to
/// say "connected" and not enough to mean it. A node whose only link was to one
/// of the nodes the crash scenario then killed was left isolated, and the
/// transaction genuinely could not reach it -- a real failure of the *harness*
/// that looked exactly like a failure of the chain.
///
/// Waiting for `len - 1` is what makes "kill any two and the rest still talk"
/// a statement about the network rather than about dial ordering.
///
/// Each pair is dialed once, from the lower index, so no two nodes dial each
/// other simultaneously. Kademlia is seeded alongside, as `network_tests.rs`
/// does, because discovery needs a starting point.
async fn connect_all(nodes: &[SimNode]) {
    for (index, node) in nodes.iter().enumerate() {
        for other in nodes.iter().skip(index + 1) {
            node.handle.dial(other.address.clone()).await.expect("dial");
            node.handle
                .add_peer_address(other.handle.peer_id(), other.address.clone())
                .await
                .expect("seed kademlia");
        }
    }

    for node in nodes {
        wait_for_peers(&node.handle, nodes.len() - 1).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_third_of_the_network_crashing_does_not_stop_the_rest() {
    // The crash-fault reading of "33% Byzantine". Dropping every `NodeHandle`
    // clone closes the command channel, and `node.rs` returns from the driver
    // on `None` — so this is a genuine stop rather than a leaked task still
    // gossiping.
    let key = generate_signing_key().expect("keygen");
    let funded = [(key.address(), 1_000_000u64)];

    let mut nodes = Vec::new();
    for _ in 0..NETWORK_SIZE {
        nodes.push(spawn_sim_node(&funded, None).await);
    }
    connect_all(&nodes).await;

    // Kill a third. `drain` moves them out so they are dropped here.
    let casualties: Vec<SimNode> = nodes.drain(0..FAULTY).collect();
    let dead_peers: Vec<_> = casualties.iter().map(|n| n.handle.peer_id()).collect();
    drop(casualties);
    assert_eq!(nodes.len(), NETWORK_SIZE - FAULTY);

    // Let the disconnects land before asserting anything about the survivors,
    // so a pass cannot be an artefact of the crash not having happened yet.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The surviving network still accepts and propagates a transaction.
    let tx = transfer(&key, [0x44u8; 32], 250, 0);
    if let Err(error) = publish_until_accepted(&nodes[0].handle, &tx).await {
        panic!("the surviving network could not publish: {error}");
    }

    let hash = tx.txid();
    let mempools: Vec<_> = nodes.iter().map(|n| n.handle.mempool().clone()).collect();
    let reached = settles(PROPAGATION_TIMEOUT, || {
        mempools.iter().all(|m| m.contains(&hash))
    })
    .await;

    assert!(
        reached,
        "a transaction did not reach all {} survivors after {FAULTY} of {NETWORK_SIZE} crashed",
        nodes.len()
    );

    report::record(
        "33% of nodes crash",
        report::Finding::Verified,
        &format!(
            "{FAULTY} of {NETWORK_SIZE} nodes dropped; the remaining {} still accepted and \
             propagated a transaction to every survivor",
            nodes.len()
        ),
    );
    let _ = dead_peers;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_third_of_the_network_broadcasting_garbage_does_not_poison_the_rest() {
    // The Byzantine reading: the faulty nodes stay up and publish hostile
    // traffic rather than going quiet. This is the harder case — a crashed node
    // sends nothing, a Byzantine one sends plausible nonsense.
    let run_seed = seed();
    println!("MAYA_CHAOS_SEED={run_seed}");
    let mut rng = SimRng::new(run_seed);

    let honest_key = generate_signing_key().expect("keygen");
    let unfunded_key = generate_signing_key().expect("keygen");
    let funded = [(honest_key.address(), 1_000_000u64)];

    let mut nodes = Vec::new();
    for _ in 0..NETWORK_SIZE {
        nodes.push(spawn_sim_node(&funded, None).await);
    }
    connect_all(&nodes).await;

    let (byzantine, honest) = nodes.split_at(FAULTY);

    // Three flavours of garbage, all of which a peer must refuse:
    //   1. a spend from an account with no balance;
    //   2. a transaction whose signature has been corrupted;
    //   3. a structurally invalid block.
    let broke = transfer(&unfunded_key, [0x55u8; 32], 10_000, 0);

    let honest_tx = transfer(&honest_key, [0x66u8; 32], 100, 0);
    let mut tampered_bytes = honest_tx.to_bytes();
    if let Some(damaged) = Corruption::FlipBit.apply(&tampered_bytes, &mut rng) {
        tampered_bytes = damaged;
    }
    let tampered = Transaction::from_bytes(&tampered_bytes).ok();

    for node in byzantine {
        for _ in 0..5 {
            let _ = node.handle.publish_transaction(&broke).await;
            if let Some(tx) = &tampered {
                let _ = node.handle.publish_transaction(tx).await;
            }
        }
    }

    // Give the garbage time to arrive and be refused.
    tokio::time::sleep(Duration::from_millis(500)).await;

    for node in honest {
        assert_eq!(
            node.handle.mempool().len(),
            0,
            "an honest node accepted Byzantine traffic into its mempool"
        );
    }

    // And the honest network still works: garbage must not have wedged it.
    if let Err(error) = publish_until_accepted(&honest[0].handle, &honest_tx).await {
        panic!("the honest network could not publish after the flood: {error}");
    }

    let hash = honest_tx.txid();
    let mempools: Vec<_> = honest.iter().map(|n| n.handle.mempool().clone()).collect();
    assert!(
        settles(PROPAGATION_TIMEOUT, || mempools
            .iter()
            .all(|m| m.contains(&hash)))
        .await,
        "an honest transaction did not propagate after the Byzantine flood"
    );

    report::record(
        "33% of nodes broadcast garbage",
        report::Finding::Verified,
        &format!(
            "{FAULTY} of {NETWORK_SIZE} published unfunded and bit-flipped transactions; \
             honest mempools stayed empty of them and an honest transaction still propagated"
        ),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gossip_stops_across_a_partition_and_resumes_when_it_heals() {
    // The delivery half of a partition, against real nodes.
    //
    // A partition is built by *never dialing across* rather than by
    // disconnecting: `NodeHandle` exposes `dial` but no disconnect, so two
    // groups that only know their own members are the honest way to model a
    // split with the API that exists.
    let key = generate_signing_key().expect("keygen");
    let funded = [(key.address(), 1_000_000u64)];

    let mut left = Vec::new();
    let mut right = Vec::new();
    for _ in 0..NETWORK_SIZE / 2 {
        left.push(spawn_sim_node(&funded, None).await);
        right.push(spawn_sim_node(&funded, None).await);
    }
    connect_all(&left).await;
    connect_all(&right).await;

    // --- partitioned --------------------------------------------------------
    let tx = transfer(&key, [0x77u8; 32], 400, 0);
    publish_until_accepted(&left[0].handle, &tx)
        .await
        .expect("publish inside the left partition");
    let hash = tx.txid();

    let left_pools: Vec<_> = left.iter().map(|n| n.handle.mempool().clone()).collect();
    assert!(
        settles(PROPAGATION_TIMEOUT, || left_pools
            .iter()
            .all(|m| m.contains(&hash)))
        .await,
        "the transaction did not reach the group it was published in"
    );

    let right_pools: Vec<_> = right.iter().map(|n| n.handle.mempool().clone()).collect();
    let crossed = settles(Duration::from_secs(2), || {
        right_pools.iter().any(|m| m.contains(&hash))
    })
    .await;
    assert!(
        !crossed,
        "the transaction crossed a partition that should have blocked it"
    );

    // --- healed -------------------------------------------------------------
    for node in &left {
        node.handle
            .dial(right[0].address.clone())
            .await
            .expect("heal");
        node.handle
            .add_peer_address(right[0].handle.peer_id(), right[0].address.clone())
            .await
            .expect("seed kademlia across the heal");
    }

    // Wait for the *new* links, not for any link at all: `right[0]` already had
    // two peers inside its own group, so waiting for one would return
    // immediately and confirm nothing. This is the count only a healed
    // partition produces.
    wait_for_peers(&right[0].handle, right.len() - 1 + left.len()).await;

    // Re-announce while waiting. Gossipsub does not replay history to a peer
    // that joins late, and a single publish into a mesh that has not finished
    // forming is dropped with an `Ok` -- so healing restores *delivery*, not
    // the messages sent while apart. That is a real property of the transport,
    // stated rather than papered over with a longer timeout.
    assert!(
        publish_until_seen(&left[0].handle, &tx, &right_pools, PROPAGATION_TIMEOUT).await,
        "the transaction did not cross after the partition healed"
    );

    report::record(
        "gossip partition and heal",
        report::Finding::Characterised,
        "delivery stops across the split and resumes on healing, but gossipsub does not \
         replay messages sent while apart -- a healed peer needs them re-published",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_five_second_latency_spike_delays_propagation_without_breaking_it() {
    // The dial is what makes this expressible: the latency is raised on
    // connections that are already open, so this tests a link degrading rather
    // than a connection that was always slow.
    let key = generate_signing_key().expect("keygen");
    let funded = [(key.address(), 1_000_000u64)];

    let dial = LatencyDial::new(Duration::ZERO);
    let a = spawn_sim_node(&funded, Some(dial.clone())).await;
    let b = spawn_sim_node(&funded, Some(dial.clone())).await;

    a.handle.dial(b.address.clone()).await.expect("dial");
    wait_for_peers(&a.handle, 1).await;
    wait_for_peers(&b.handle, 1).await;

    // --- baseline -----------------------------------------------------------
    let first = transfer(&key, [0x88u8; 32], 100, 0);
    publish_until_accepted(&a.handle, &first)
        .await
        .expect("baseline publish");
    let first_hash = first.txid();
    let b_pool = b.handle.mempool().clone();
    assert!(
        settles(PROPAGATION_TIMEOUT, || b_pool.contains(&first_hash)).await,
        "propagation failed before any spike"
    );

    // --- spike --------------------------------------------------------------
    dial.set(SPIKE);
    assert_eq!(dial.get(), SPIKE);

    let second = transfer(&key, [0x88u8; 32], 100, 1);
    let published = publish_until_accepted(&a.handle, &second).await.is_ok();
    let second_hash = second.txid();

    // The connection must survive the spike. Whether the message arrives inside
    // the window is a race against a 5 s per-read delay, so the assertion is
    // that nothing is torn down -- not a timing claim the test cannot honestly
    // make.
    let arrived_during_spike =
        published && settles(Duration::from_secs(12), || b_pool.contains(&second_hash)).await;

    // --- recovery -----------------------------------------------------------
    dial.set(Duration::ZERO);

    assert!(
        settles(PROPAGATION_TIMEOUT, || b_pool.contains(&second_hash)).await,
        "propagation did not recover after the spike was cleared"
    );
    assert!(
        a.handle
            .connected_peers()
            .await
            .map(|p| !p.is_empty())
            .unwrap_or(false),
        "the peer connection did not survive the spike"
    );

    report::record(
        "5000 ms latency spike",
        report::Finding::Verified,
        &format!(
            "latency raised to {} ms on open connections and cleared; the connection \
             survived and propagation resumed (delivered during the spike: {arrived_during_spike})",
            SPIKE.as_millis()
        ),
    );
}

// ---------------------------------------------------------------------------
// The report
// ---------------------------------------------------------------------------

#[test]
fn zz_write_the_resilience_report() {
    // Named `zz_` so it sorts last under `cargo test`, which runs tests in
    // name order within a binary. Under `cargo nextest` every test is its own
    // process and ordering is not guaranteed at all -- which is why the rows
    // are accumulated on disk rather than in memory, and why this test is
    // tolerant of finding fewer of them than a full run produces.
    let run_seed = seed();
    match report::write(run_seed).expect("write the report") {
        Some(path) => println!("resilience report: {}", path.display()),
        None => println!("no scenarios recorded; report not written"),
    }
}
