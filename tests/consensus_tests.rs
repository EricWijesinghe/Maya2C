//! Consensus tests: 256-bit arithmetic, difficulty retargeting, cumulative
//! work, undo journaling, and chain reorganization.
//!
//! Chain tests run with proof-of-work verification disabled. Each verification
//! is a 32 MiB Argon2id pass, and these tests care about fork choice and state
//! consistency, not about whether a nonce was actually ground out. Mining is
//! covered separately by the crypto suite.

use std::sync::Arc;

use custom_l1_node::consensus::difficulty::{
    EXPECTED_TIMESPAN, MAX_ADJUSTMENT_FACTOR, RETARGET_INTERVAL, TARGET_BLOCK_TIME, clamp_timespan,
    default_pow_limit, unlimited_pow_limit,
};
use custom_l1_node::consensus::uint::U256;
use custom_l1_node::consensus::{
    BlockId, Chain, ChainConfig, InsertOutcome, is_retarget_height, retarget, work_from_target,
};
use custom_l1_node::core::{Block, BlockHeader, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::generate_signing_key;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};

use custom_l1_node::crypto::hybrid::HybridSigningKey;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn open_state() -> (Arc<StateDB>, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open state");
    (Arc::new(db), dir)
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

/// Genesis at a trivially easy target, so inherited targets stay easy.
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

/// Builds a child of `parent` carrying the target the chain rules require.
///
/// `timestamp` also serves to distinguish sibling blocks: it feeds the header
/// hash, so two branches built from the same parent get different ids.
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

/// A second node with the same genesis state as a test's main chain, funding
/// `address` with `balance`: where a competing branch is mined.
fn funded_shadow(address: &Address, balance: u64) -> (Chain, TempDir) {
    let (state, dir) = open_state();
    state
        .put_account(address, &Account { balance, nonce: 0 })
        .expect("fund the shadow");
    (test_chain(state), dir)
}

// ---------------------------------------------------------------------------
// U256
// ---------------------------------------------------------------------------

#[test]
fn u256_byte_roundtrip_preserves_value() {
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = index as u8;
    }
    assert_eq!(U256::from_be_bytes(&bytes).to_be_bytes(), bytes);
}

#[test]
fn u256_orders_by_magnitude() {
    let small = U256::from_u64(5);
    let large = U256::from_u64(u64::MAX);
    let huge = U256::from_be_bytes(&[0xFFu8; 32]);

    assert!(small < large);
    assert!(large < huge);
    assert_eq!(huge, U256::MAX);
    assert!(U256::ZERO.is_zero());
}

#[test]
fn u256_mul_div_is_exact() {
    let value = U256::from_u64(1_000);
    let result = value.mul_div_u64(3, 2).expect("fits");
    assert_eq!(result, U256::from_u64(1_500));

    // Multiply beyond 64 bits, then divide back down.
    let big = U256::from_u64(u64::MAX);
    let doubled = big.mul_div_u64(2, 1).expect("fits in 256 bits");
    let halved = doubled.mul_div_u64(1, 2).expect("fits");
    assert_eq!(halved, big);
}

#[test]
fn u256_mul_div_rejects_a_zero_divisor() {
    assert_eq!(U256::from_u64(10).mul_div_u64(1, 0), None);
}

#[test]
fn u256_div_rem_matches_known_values() {
    let (quotient, remainder) = U256::from_u64(17)
        .div_rem(U256::from_u64(5))
        .expect("non-zero divisor");
    assert_eq!(quotient, U256::from_u64(3));
    assert_eq!(remainder, U256::from_u64(2));

    // Exact division leaves no remainder.
    let (q, r) = U256::from_u64(100)
        .div_rem(U256::from_u64(4))
        .expect("non-zero divisor");
    assert_eq!(q, U256::from_u64(25));
    assert!(r.is_zero());

    assert_eq!(U256::from_u64(1).div_rem(U256::ZERO), None);
}

// ---------------------------------------------------------------------------
// difficulty retargeting
// ---------------------------------------------------------------------------

#[test]
fn retarget_window_constants_are_consistent() {
    assert_eq!(RETARGET_INTERVAL, 100);
    assert_eq!(TARGET_BLOCK_TIME, 15);
    assert_eq!(EXPECTED_TIMESPAN, 1_500);
    assert_eq!(MAX_ADJUSTMENT_FACTOR, 4);
}

#[test]
fn on_schedule_window_leaves_difficulty_unchanged() {
    let target = target_from_leading_zero_bits(20);
    assert_eq!(
        retarget(&target, EXPECTED_TIMESPAN, &unlimited_pow_limit()),
        target
    );
}

#[test]
fn slow_window_eases_difficulty() {
    let target = target_from_leading_zero_bits(20);
    let eased = retarget(&target, EXPECTED_TIMESPAN * 2, &unlimited_pow_limit());

    // A larger target is easier to satisfy.
    assert!(U256::from_be_bytes(&eased) > U256::from_be_bytes(&target));
    assert_eq!(
        U256::from_be_bytes(&eased),
        U256::from_be_bytes(&target)
            .mul_div_u64(2, 1)
            .expect("fits")
    );
}

#[test]
fn fast_window_tightens_difficulty() {
    let target = target_from_leading_zero_bits(20);
    let tightened = retarget(&target, EXPECTED_TIMESPAN / 2, &unlimited_pow_limit());

    assert!(U256::from_be_bytes(&tightened) < U256::from_be_bytes(&target));
    assert_eq!(
        U256::from_be_bytes(&tightened),
        U256::from_be_bytes(&target)
            .mul_div_u64(1, 2)
            .expect("fits")
    );
}

#[test]
fn adjustment_is_clamped_to_four_times_in_both_directions() {
    // An absurd timespan must not move difficulty further than the 4x bound.
    assert_eq!(clamp_timespan(u64::MAX), EXPECTED_TIMESPAN * 4);
    assert_eq!(clamp_timespan(0), EXPECTED_TIMESPAN / 4);
    assert_eq!(clamp_timespan(EXPECTED_TIMESPAN), EXPECTED_TIMESPAN);

    let target = target_from_leading_zero_bits(20);
    let base = U256::from_be_bytes(&target);

    let max_eased = U256::from_be_bytes(&retarget(
        &target,
        EXPECTED_TIMESPAN * 1_000,
        &unlimited_pow_limit(),
    ));
    assert_eq!(max_eased, base.mul_div_u64(4, 1).expect("fits"));

    let max_tightened = U256::from_be_bytes(&retarget(&target, 1, &unlimited_pow_limit()));
    assert_eq!(max_tightened, base.mul_div_u64(1, 4).expect("fits"));
}

#[test]
fn retarget_never_eases_past_the_pow_limit() {
    let limit = default_pow_limit();
    // Already at the floor; a slow window cannot push difficulty below it.
    let eased = retarget(&limit, EXPECTED_TIMESPAN * 4, &limit);
    assert_eq!(eased, limit);
}

#[test]
fn retarget_heights_land_on_interval_boundaries() {
    assert!(!is_retarget_height(0), "genesis never retargets");
    assert!(!is_retarget_height(1));
    assert!(!is_retarget_height(99));
    assert!(is_retarget_height(100));
    assert!(!is_retarget_height(101));
    assert!(is_retarget_height(200));
}

// ---------------------------------------------------------------------------
// cumulative work
// ---------------------------------------------------------------------------

#[test]
fn harder_targets_carry_more_work() {
    let easy = work_from_target(&target_from_leading_zero_bits(8));
    let medium = work_from_target(&target_from_leading_zero_bits(16));
    let hard = work_from_target(&target_from_leading_zero_bits(24));

    assert!(easy < medium);
    assert!(medium < hard);
}

#[test]
fn work_of_the_easiest_target_is_one() {
    // Every digest satisfies an all-ones target, so it costs one hash.
    assert_eq!(work_from_target(&[0xFFu8; 32]), U256::ONE);
}

#[test]
fn work_doubles_when_the_target_halves() {
    let target = target_from_leading_zero_bits(16);
    let harder = target_from_leading_zero_bits(17);

    let work = work_from_target(&target);
    let harder_work = work_from_target(&harder);

    // One extra required zero bit halves the target, doubling expected hashes.
    assert_eq!(harder_work, work.mul_div_u64(2, 1).expect("fits"));
}

// ---------------------------------------------------------------------------
// undo journal
// ---------------------------------------------------------------------------

#[test]
fn reverting_a_block_restores_state_exactly() {
    let (state, _dir) = open_state();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = [7u8; 32];

    state
        .put_account(
            &alice_addr,
            &Account {
                balance: 1_000,
                nonce: 0,
            },
        )
        .expect("fund");

    let root_before = state.state_root().expect("root");
    let mut block = Block::new(genesis().header, vec![transfer(&alice, bob_addr, 250, 0)]);
    // The journaled path checks the declared root, so declare the real one.
    block.header.state_root = state
        .preview_root(&block, BlockContext::GENESIS)
        .expect("preview");
    let block_id = [1u8; 32];

    state
        .apply_block_journaled(&block, &block_id, BlockContext::GENESIS)
        .expect("apply");
    assert_eq!(state.get_account(&alice_addr).map(|a| a.balance), Ok(750));
    assert_eq!(state.get_account(&bob_addr).map(|a| a.balance), Ok(250));
    assert!(state.has_undo(&block_id).expect("undo present"));

    state.revert_block(&block_id).expect("revert");

    assert_eq!(
        state.get_account(&alice_addr),
        Ok(Account {
            balance: 1_000,
            nonce: 0
        })
    );
    // Bob never existed before the block; he must be deleted, not zeroed, or
    // the state root would not match.
    assert_eq!(state.get_account(&bob_addr), Ok(Account::default()));
    assert_eq!(state.state_root(), Ok(root_before));
    assert!(!state.has_undo(&block_id).expect("undo consumed"));
}

#[test]
fn reverting_an_unknown_block_is_an_error() {
    let (state, _dir) = open_state();
    assert!(state.revert_block(&[9u8; 32]).is_err());
}

// ---------------------------------------------------------------------------
// chain: extension and fork choice
// ---------------------------------------------------------------------------

#[test]
fn blocks_extend_the_active_chain() {
    let (state, _dir) = open_state();
    let mut chain = test_chain(state);
    let genesis_id = chain.genesis();

    let block = child_of(&chain, genesis_id, 1_000_015, vec![]);
    let id = block.header.id();

    assert_eq!(
        chain.insert_block(block),
        Ok(InsertOutcome::Extended { tip: id })
    );
    assert_eq!(chain.tip(), id);
    assert_eq!(chain.height(), 1);
}

#[test]
fn a_duplicate_block_changes_nothing() {
    let (state, _dir) = open_state();
    let mut chain = test_chain(state);
    let block = child_of(&chain, chain.genesis(), 1_000_015, vec![]);
    let id = block.header.id();

    chain.insert_block(block.clone()).expect("first insert");
    assert_eq!(
        chain.insert_block(block),
        Ok(InsertOutcome::Duplicate { id })
    );
    assert_eq!(chain.height(), 1);
}

#[test]
fn a_block_with_an_unknown_parent_is_rejected() {
    let (state, _dir) = open_state();
    let mut chain = test_chain(state);

    let orphan = Block::new(
        BlockHeader {
            prev_hash: [0xAAu8; 32],
            state_root: [0u8; 32],
            timestamp: 1_000_015,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        vec![],
    );

    assert!(chain.insert_block(orphan).is_err());
}

#[test]
fn a_block_declaring_the_wrong_target_is_rejected() {
    let (state, _dir) = open_state();
    let mut chain = test_chain(state);

    // Claim an easier target than the rules allow.
    let mut block = child_of(&chain, chain.genesis(), 1_000_015, vec![]);
    block.header.difficulty_target = target_from_leading_zero_bits(1);

    assert!(chain.insert_block(block).is_err());
    assert_eq!(chain.height(), 0);
}

#[test]
fn an_equal_work_branch_does_not_displace_the_incumbent_tip() {
    let (state, _dir) = open_state();
    let mut chain = test_chain(state);
    let genesis_id = chain.genesis();

    let first = child_of(&chain, genesis_id, 1_000_015, vec![]);
    let first_id = first.header.id();
    chain.insert_block(first).expect("insert first");

    // Same height, same target, therefore same cumulative work.
    let sibling = child_of(&chain, genesis_id, 1_000_016, vec![]);
    let sibling_id = sibling.header.id();

    assert_eq!(
        chain.insert_block(sibling),
        Ok(InsertOutcome::SideBranch { id: sibling_id })
    );
    assert_eq!(chain.tip(), first_id, "tie must keep the incumbent");
}

// ---------------------------------------------------------------------------
// chain: reorganization
// ---------------------------------------------------------------------------

#[test]
fn a_heavier_branch_triggers_a_reorg_and_rewrites_state() {
    let (state, _dir) = open_state();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = [7u8; 32];
    let carol_addr = [8u8; 32];

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
    let genesis_id = chain.genesis();

    // Branch A: two blocks, one spend of 100 to Bob.
    let a1 = child_of(
        &chain,
        genesis_id,
        1_000_015,
        vec![transfer(&alice, bob_addr, 100, 0)],
    );
    let a1_id = a1.header.id();
    chain.insert_block(a1).expect("a1");
    let a2 = child_of(&chain, a1_id, 1_000_030, vec![]);
    let a2_id = a2.header.id();
    chain.insert_block(a2).expect("a2");

    assert_eq!(chain.tip(), a2_id);
    assert_eq!(state.get_account(&alice_addr).map(|a| a.balance), Ok(900));
    assert_eq!(state.get_account(&bob_addr).map(|a| a.balance), Ok(100));

    // Branch B: three blocks spending to Carol instead. Same nonces as branch
    // A, which is exactly the cross-fork double spend a reorg must resolve.
    //
    // Minted on a second node holding the same genesis state, as a competing
    // miner would: each block declares the root it executes to, and only a
    // node whose tip is the block's parent can compute that.
    let (mut shadow, _shadow_dir) = funded_shadow(&alice_addr, 1_000);
    let b1 = child_of(
        &shadow,
        genesis_id,
        1_000_020,
        vec![transfer(&alice, carol_addr, 300, 0)],
    );
    let b1_id = b1.header.id();
    shadow.insert_block(b1.clone()).expect("b1 on the shadow");
    assert!(matches!(
        chain.insert_block(b1),
        Ok(InsertOutcome::SideBranch { .. })
    ));

    let b2 = child_of(
        &shadow,
        b1_id,
        1_000_035,
        vec![transfer(&alice, carol_addr, 200, 1)],
    );
    let b2_id = b2.header.id();
    shadow.insert_block(b2.clone()).expect("b2 on the shadow");
    assert!(
        matches!(chain.insert_block(b2), Ok(InsertOutcome::SideBranch { .. })),
        "equal work must not reorg"
    );
    assert_eq!(chain.tip(), a2_id);

    // Third block tips the balance of cumulative work.
    let b3 = child_of(&shadow, b2_id, 1_000_050, vec![]);
    let b3_id = b3.header.id();

    let outcome = chain.insert_block(b3).expect("b3");
    match outcome {
        InsertOutcome::Reorganized {
            tip,
            reverted,
            applied,
        } => {
            assert_eq!(tip, b3_id);
            // Newest first.
            assert_eq!(reverted, vec![a2_id, a1_id]);
            // Oldest first.
            assert_eq!(applied, vec![b1_id, b2_id, b3_id]);
        }
        other => panic!("expected a reorg, got {other:?}"),
    }

    assert_eq!(chain.tip(), b3_id);
    assert_eq!(chain.height(), 3);

    // State now reflects branch B only. Bob's account never existed on B, so
    // the undo journal deleted it rather than leaving a zero balance behind.
    assert_eq!(
        state.get_account(&alice_addr),
        Ok(Account {
            balance: 500,
            nonce: 2
        })
    );
    assert_eq!(state.get_account(&carol_addr).map(|a| a.balance), Ok(500));
    assert_eq!(state.get_account(&bob_addr), Ok(Account::default()));
}

#[test]
fn a_reorg_that_fails_midway_restores_the_original_chain() {
    let (state, _dir) = open_state();
    let alice = generate_signing_key().expect("keygen");
    let mallory = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = [7u8; 32];

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
    let genesis_id = chain.genesis();

    let a1 = child_of(
        &chain,
        genesis_id,
        1_000_015,
        vec![transfer(&alice, bob_addr, 100, 0)],
    );
    let a1_id = a1.header.id();
    chain.insert_block(a1).expect("a1");
    let a2 = child_of(&chain, a1_id, 1_000_030, vec![]);
    let a2_id = a2.header.id();
    chain.insert_block(a2).expect("a2");

    let root_before = state.state_root().expect("root");

    // Branch B is heavier but its last block spends funds Mallory never had.
    // Minted on a shadow node with the same genesis state, so its first two
    // blocks declare real roots and it is the spend that fails the reorg.
    let (mut shadow, _shadow_dir) = funded_shadow(&alice_addr, 1_000);
    let b1 = child_of(&shadow, genesis_id, 1_000_020, vec![]);
    let b1_id = b1.header.id();
    shadow.insert_block(b1.clone()).expect("b1 on the shadow");
    chain.insert_block(b1).expect("b1");
    let b2 = child_of(&shadow, b1_id, 1_000_035, vec![]);
    let b2_id = b2.header.id();
    shadow.insert_block(b2.clone()).expect("b2 on the shadow");
    chain.insert_block(b2).expect("b2");

    let b3 = child_of(
        &shadow,
        b2_id,
        1_000_050,
        vec![transfer(&mallory, bob_addr, 5_000, 0)],
    );

    assert!(
        chain.insert_block(b3).is_err(),
        "a block spending nonexistent funds must be rejected"
    );

    // The failed reorg must leave no trace: same tip, same state, same root.
    assert_eq!(chain.tip(), a2_id, "tip must survive a failed reorg");
    assert_eq!(chain.height(), 2);
    assert_eq!(state.get_account(&alice_addr).map(|a| a.balance), Ok(900));
    assert_eq!(state.get_account(&bob_addr).map(|a| a.balance), Ok(100));
    assert_eq!(state.state_root(), Ok(root_before));
}

#[test]
fn active_chain_lists_blocks_from_genesis_to_tip() {
    let (state, _dir) = open_state();
    let mut chain = test_chain(state);
    let genesis_id = chain.genesis();

    let first = child_of(&chain, genesis_id, 1_000_015, vec![]);
    let first_id = first.header.id();
    chain.insert_block(first).expect("first");

    let second = child_of(&chain, first_id, 1_000_030, vec![]);
    let second_id = second.header.id();
    chain.insert_block(second).expect("second");

    assert_eq!(
        chain.active_chain(),
        Ok(vec![genesis_id, first_id, second_id])
    );
}

// ---------------------------------------------------------------------------
// retargeting across a real window
// ---------------------------------------------------------------------------

#[test]
fn difficulty_recalculates_at_the_hundredth_block() {
    let (state, _dir) = open_state();
    let mut chain = test_chain(state);

    let genesis_target = target_from_leading_zero_bits(0);
    let mut parent = chain.genesis();
    let mut timestamp = 1_000_000u64;

    // Blocks 1..=99 inherit the genesis target unchanged.
    for _ in 1..RETARGET_INTERVAL {
        timestamp += TARGET_BLOCK_TIME;
        let block = child_of(&chain, parent, timestamp, vec![]);
        assert_eq!(
            block.header.difficulty_target, genesis_target,
            "mid-window blocks must inherit the target"
        );
        parent = block.header.id();
        chain.insert_block(block).expect("insert");
    }
    assert_eq!(chain.height(), RETARGET_INTERVAL - 1);

    // Block 100 is the first retarget height.
    timestamp += TARGET_BLOCK_TIME;
    let retargeted = child_of(&chain, parent, timestamp, vec![]);
    assert!(
        chain.height() + 1 == RETARGET_INTERVAL && is_retarget_height(RETARGET_INTERVAL),
        "block 100 should be a retarget height"
    );

    // The window spanned 99 intervals, slightly under the intended 100, so the
    // target tightens proportionally.
    let window = 99 * TARGET_BLOCK_TIME;
    let expected = retarget(&genesis_target, window, &unlimited_pow_limit());
    assert_eq!(retargeted.header.difficulty_target, expected);
    assert!(
        U256::from_be_bytes(&retargeted.header.difficulty_target)
            < U256::from_be_bytes(&genesis_target),
        "a faster-than-scheduled window must tighten difficulty"
    );

    chain.insert_block(retargeted).expect("insert retargeted");
    assert_eq!(chain.height(), RETARGET_INTERVAL);
}
