//! Behaviour under adversarial conditions.
//!
//! # What each of these is, and is not
//!
//! All of them are **verification** tests: the chain has a defence and this
//! checks it works. The sybil test used to be a **characterisation** — it
//! recorded that no peer scoring or connection limit existed — and was replaced
//! when the peer guard landed (`docs/peer-health.md`,
//! `byzantine_guard_tests.rs`).
//!
//! The distinction is the point. A green tick next to "sybil flood" that
//! implied a defence existed would be worse than no test at all, which is why
//! that one said so in its name until the defence was real.
//!
//! # "51% hash rate drop" is two different events
//!
//! The brief named one thing; these are two, and they fail differently:
//!
//! - A **hashrate drop** is an honest event — miners leave. Blocks slow until
//!   the next retarget, and the chain's protection is the retarget clamp.
//! - A **51% attack** is an adversary with majority hashrate building a heavier
//!   branch and reorganising. The chain's protection is that fork choice
//!   compares accumulated work, not length.
//!
//! Both are covered below, separately.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::BlockId;
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::consensus::difficulty::{
    MAX_ADJUSTMENT_FACTOR, RETARGET_INTERVAL, TARGET_BLOCK_TIME, cumulative_work,
    default_pow_limit, retarget,
};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::BlockContext;
use custom_l1_node::state::account::{Account, Address};
use custom_l1_node::state::db::StateDB;
use tempfile::TempDir;

fn open_state() -> (Arc<StateDB>, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open state");
    (Arc::new(db), dir)
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
    // refuses any other. Only computable on the tip; a side-branch block is
    // minted on a node whose tip is its parent. A block that cannot execute
    // keeps the zero root, and the chain refuses it for its transactions
    // before any root is compared.
    if parent == chain.tip()
        && let Ok(root) = chain
            .state()
            .preview_root(&block, BlockContext::at_height(chain.height() + 1))
    {
        block.header.state_root = root;
    }
    block
}

fn test_chain(state: Arc<StateDB>) -> Chain {
    Chain::open(state, genesis(), ChainConfig::without_pow_verification()).expect("open chain")
}

// ---------------------------------------------------------------------------
// Double-spend race
// ---------------------------------------------------------------------------

#[test]
fn only_one_of_two_conflicting_spends_can_be_applied() {
    // The core double-spend case: one account, one balance, two transactions
    // that each spend it. Whichever lands first must make the other invalid —
    // not by policy, but because the nonce has moved and the balance is gone.
    let (state, _dir) = open_state();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = alice.address();

    state
        .put_account(
            &alice_addr,
            &Account {
                balance: 1_000,
                nonce: 0,
            },
        )
        .expect("fund");

    let mut chain = test_chain(Arc::clone(&state));
    let tip = chain.tip();

    // Two spends of the same funds at the same nonce, to different recipients.
    let to_bob = transfer(&alice, [7u8; 32], 900, 0);
    let to_carol = transfer(&alice, [8u8; 32], 900, 0);
    assert_ne!(
        to_bob.txid(),
        to_carol.txid(),
        "the two spends must be distinct transactions"
    );

    let block = child_of(&chain, tip, 1_000_015, vec![to_bob]);
    chain
        .insert_block(block)
        .expect("the first spend is accepted");

    // The second, in a later block, must now fail: the nonce advanced and the
    // balance is spent.
    let tip = chain.tip();
    let second = child_of(&chain, tip, 1_000_030, vec![to_carol]);
    assert!(
        chain.insert_block(second).is_err(),
        "a second spend of the same funds at the same nonce must be refused"
    );

    let alice_after = state.get_account(&alice_addr).expect("read");
    assert_eq!(
        alice_after.balance, 100,
        "exactly one spend may have landed"
    );
    assert_eq!(alice_after.nonce, 1);
}

#[test]
fn two_conflicting_spends_in_one_block_fail_the_whole_block() {
    // Both in one block. A failing transaction fails its whole block here —
    // which is why `docs/dex.md` insists a merely-losing trade must be a no-op
    // rather than an error. The same rule makes this atomic: the block is
    // rejected entire, not partially applied.
    let (state, _dir) = open_state();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = alice.address();

    state
        .put_account(
            &alice_addr,
            &Account {
                balance: 1_000,
                nonce: 0,
            },
        )
        .expect("fund");

    let mut chain = test_chain(Arc::clone(&state));
    let tip = chain.tip();

    let block = child_of(
        &chain,
        tip,
        1_000_015,
        vec![
            transfer(&alice, [7u8; 32], 900, 0),
            transfer(&alice, [8u8; 32], 900, 0),
        ],
    );
    assert!(
        chain.insert_block(block).is_err(),
        "a block containing a double spend must be rejected entire"
    );

    let after = state.get_account(&alice_addr).expect("read");
    assert_eq!(
        after.balance, 1_000,
        "a rejected block must leave no trace: neither spend may have applied"
    );
    assert_eq!(after.nonce, 0);
}

// ---------------------------------------------------------------------------
// Majority hashrate: the 51% attack
// ---------------------------------------------------------------------------

#[test]
fn a_longer_branch_with_less_work_does_not_win() {
    // The property that makes a 51% attack expensive rather than merely
    // patient: fork choice sums *work*, not blocks.
    //
    // # Why this is tested here and not by building two branches
    //
    // It cannot be built at the chain level inside one retarget window. A block
    // whose `difficulty_target` differs from what `Chain::next_target` derives
    // is rejected outright — an attacker cannot simply declare an easier
    // target, which is itself the first line of defence. Constructing branches
    // of genuinely different difficulty needs to cross a retarget boundary, and
    // that is RETARGET_INTERVAL blocks of setup to exercise one comparison.
    //
    // So the comparison is made against `cumulative_work`, which is the
    // quantity `Chain::total_work` uses and therefore the thing fork choice
    // actually decides on. The chain-level behaviour — that a heavier branch
    // takes the tip — is already covered by
    // `consensus_tests::a_heavier_branch_triggers_a_reorg_and_rewrites_state`.
    let hard = target_from_leading_zero_bits(20);
    let easy = target_from_leading_zero_bits(1);

    let honest = cumulative_work([&hard]);
    // Sixty-four blocks on the attacker's branch against the honest one.
    let attacker_targets = vec![easy; 64];
    let attacker = cumulative_work(attacker_targets.iter());

    assert!(
        honest > attacker,
        "one block at 20 leading zero bits must outweigh 64 at 1: \
         honest {honest:?}, attacker {attacker:?}"
    );
}

#[test]
fn an_easier_target_is_worth_proportionally_less() {
    // The arithmetic the previous test rests on, stated directly. Each extra
    // leading zero bit doubles the expected hashes, so work doubles too. If
    // this relationship were ever wrong, fork choice would misprice branches
    // and the comparison above would pass while meaning nothing.
    let base = target_from_leading_zero_bits(16);
    let harder = target_from_leading_zero_bits(17);

    let base_work = cumulative_work([&base]);
    let harder_work = cumulative_work([&harder]);

    assert!(
        harder_work > base_work,
        "a harder target must be worth more"
    );

    // Two blocks at the easier target should be worth about one at the harder.
    let two_easy = cumulative_work([&base, &base]);
    assert!(
        two_easy >= harder_work,
        "one extra zero bit should be worth roughly two blocks: \
         two_easy {two_easy:?}, harder {harder_work:?}"
    );
}

// ---------------------------------------------------------------------------
// Hashrate drop: the retarget clamp
// ---------------------------------------------------------------------------

#[test]
fn a_hashrate_collapse_cannot_ease_difficulty_faster_than_the_clamp() {
    // An honest event, not an attack: half the miners leave and blocks slow.
    // The chain's protection is that one window cannot move difficulty
    // arbitrarily far — otherwise a single manipulated timespan, or one genuine
    // cliff, would swing it wildly.
    let target = target_from_leading_zero_bits(24);
    let expected_span = RETARGET_INTERVAL * TARGET_BLOCK_TIME;
    // The floor the retarget is clamped against. Passed explicitly because it
    // is a chain parameter, not a constant — a test network's floor is far
    // easier than mainnet's, and `retarget` must never ease past whichever one
    // it was given.
    let pow_limit = default_pow_limit();

    // A window that took a hundred times as long as intended.
    let eased = retarget(&target, expected_span * 100, &pow_limit);

    // And one that took the full clamp.
    let clamped = retarget(&target, expected_span * MAX_ADJUSTMENT_FACTOR, &pow_limit);

    assert_eq!(
        eased, clamped,
        "an extreme timespan must land on the clamp, not past it"
    );
}

#[test]
fn a_hashrate_surge_cannot_harden_difficulty_faster_than_the_clamp() {
    // The other direction, which matters more for an attacker: a miner who
    // manipulates timestamps to make a window look fast cannot spike difficulty
    // to lock others out.
    let target = target_from_leading_zero_bits(24);
    let expected_span = RETARGET_INTERVAL * TARGET_BLOCK_TIME;
    let pow_limit = default_pow_limit();

    let hardened = retarget(&target, 1, &pow_limit);
    let clamped = retarget(&target, expected_span / MAX_ADJUSTMENT_FACTOR, &pow_limit);

    assert_eq!(
        hardened, clamped,
        "a near-zero timespan must land on the clamp, not past it"
    );
}

// ---------------------------------------------------------------------------
// Sybil: peer scoring, quarantine and connection limits
// ---------------------------------------------------------------------------

#[test]
fn peer_scoring_quarantine_and_connection_limits_are_wired() {
    // Was `characterisation_there_is_no_peer_limit_or_scoring`, which recorded
    // that none of this existed and asked to be replaced once it did.
    //
    // The behaviour — hostile peers quarantined and cut off, honest and slow
    // ones not — is verified against running nodes in
    // `byzantine_guard_tests.rs`. This pins the configuration a sybil flood
    // meets, so that removing any piece of it fails here as well as there.
    let behaviour_source = include_str!("../src/network/behaviour.rs");

    for marker in [
        "validate_messages",
        "with_peer_score",
        "ConnectionLimits",
        "with_max_established_per_peer",
        "block_list",
    ] {
        assert!(
            behaviour_source.contains(marker),
            "`{marker}` is no longer configured in behaviour.rs — the peer guard \
             has lost a piece. See docs/peer-health.md."
        );
    }
}

#[test]

mod common;
fn a_sybil_flood_cannot_forge_a_heavier_chain() {
    // The reassurance that matters despite the gap above.
    //
    // Sybil identities are cheap; proof of work is not. An adversary can
    // surround a node with any number of identities and still cannot make it
    // accept a chain carrying less work, because fork choice never counts peers
    // or blocks — it sums difficulty. Identity is not a vote here.
    //
    // So the missing peer scoring is an availability and eclipse concern, not a
    // consensus one, and those want different fixes.
    let honest = cumulative_work([&target_from_leading_zero_bits(20)]);

    // A thousand identities, each contributing a minimum-difficulty block.
    let flood = vec![target_from_leading_zero_bits(1); 1_000];
    let flooded = cumulative_work(flood.iter());

    assert!(
        honest > flooded,
        "no number of minimum-difficulty blocks may outweigh real work: \
         honest {honest:?}, flood of {} {flooded:?}",
        flood.len()
    );
}
