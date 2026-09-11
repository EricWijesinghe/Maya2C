//! Integration tests for RocksDB-backed state: transition rules,
//! double-spend rejection, nonce enforcement, and batch rollback.

use custom_l1_node::core::{Block, BlockHeader, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::generate_signing_key;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};

use custom_l1_node::crypto::hybrid::HybridSigningKey;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Opens a StateDB in a fresh temp directory. The directory is returned so the
/// caller keeps it alive — dropping it deletes the database.
fn open_db() -> (StateDB, TempDir) {
    let dir = TempDir::new().expect("temp dir");
    let db = StateDB::open(dir.path()).expect("open state db");
    (db, dir)
}

fn address_of(key: &HybridSigningKey) -> Address {
    key.address()
}

fn fund(db: &StateDB, address: &Address, balance: u64) {
    db.put_account(address, &Account { balance, nonce: 0 })
        .expect("fund account");
}

/// Builds a signed transfer of `amount` from `from` to `to` at `nonce`.
fn transfer(from: &HybridSigningKey, to: &Address, amount: u64, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount,
            recipient: *to,
        }],
        nonce,
    );
    tx.sign(from).expect("sign");
    tx
}

fn block_of(transactions: Vec<Transaction>) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        transactions,
    )
}

// ---------------------------------------------------------------------------
// basic persistence
// ---------------------------------------------------------------------------

#[test]
fn unknown_address_reads_as_empty_account() {
    let (db, _dir) = open_db();
    assert_eq!(db.get_account(&[9u8; 32]), Ok(Account::default()));
}

#[test]
fn accounts_survive_reopen() {
    let dir = TempDir::new().expect("temp dir");
    let address = [4u8; 32];

    {
        let db = StateDB::open(dir.path()).expect("open");
        fund(&db, &address, 500);
    } // dropped: db closed, files remain

    let reopened = StateDB::open(dir.path()).expect("reopen");
    assert_eq!(
        reopened.get_account(&address),
        Ok(Account {
            balance: 500,
            nonce: 0
        })
    );
}

// ---------------------------------------------------------------------------
// state transition rules
// ---------------------------------------------------------------------------

#[test]
fn valid_transfer_moves_balance_and_advances_nonce() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = [2u8; 32];

    fund(&db, &alice_addr, 1_000);

    let block = block_of(vec![transfer(&alice, &bob_addr, 400, 0)]);
    db.apply_block(&block, BlockContext::GENESIS)
        .expect("block should apply");

    assert_eq!(
        db.get_account(&alice_addr),
        Ok(Account {
            balance: 600,
            nonce: 1
        })
    );
    assert_eq!(
        db.get_account(&bob_addr),
        Ok(Account {
            balance: 400,
            nonce: 0
        })
    );
}

#[test]
fn sequential_transfers_in_one_block_both_apply() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = [2u8; 32];

    fund(&db, &alice_addr, 1_000);

    // Nonces 0 then 1: the second reads the first's effect from the overlay.
    let block = block_of(vec![
        transfer(&alice, &bob_addr, 100, 0),
        transfer(&alice, &bob_addr, 250, 1),
    ]);
    db.apply_block(&block, BlockContext::GENESIS)
        .expect("block should apply");

    assert_eq!(
        db.get_account(&alice_addr),
        Ok(Account {
            balance: 650,
            nonce: 2
        })
    );
    assert_eq!(db.get_account(&bob_addr).map(|a| a.balance), Ok(350));
}

#[test]
fn self_transfer_nets_to_zero_and_does_not_mint() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);

    fund(&db, &alice_addr, 1_000);

    let block = block_of(vec![transfer(&alice, &alice_addr, 750, 0)]);
    db.apply_block(&block, BlockContext::GENESIS)
        .expect("block should apply");

    // Debit then credit on the same account must conserve value.
    assert_eq!(
        db.get_account(&alice_addr),
        Ok(Account {
            balance: 1_000,
            nonce: 1
        })
    );
}

// ---------------------------------------------------------------------------
// double spending
// ---------------------------------------------------------------------------

#[test]
fn double_spend_within_one_block_is_rejected() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = [2u8; 32];

    fund(&db, &alice_addr, 1_000);

    // Both transactions reuse nonce 0. The first advances the overlay nonce to
    // 1, so the replay no longer matches.
    let block = block_of(vec![
        transfer(&alice, &bob_addr, 600, 0),
        transfer(&alice, &bob_addr, 600, 0),
    ]);

    assert_eq!(
        db.apply_block(&block, BlockContext::GENESIS),
        Err(NodeError::InvalidNonce {
            address: hex::encode(alice_addr),
            expected: 1,
            actual: 0,
        })
    );

    // Nothing committed — not even the first, valid transaction.
    assert_eq!(
        db.get_account(&alice_addr),
        Ok(Account {
            balance: 1_000,
            nonce: 0
        })
    );
    assert_eq!(db.get_account(&bob_addr), Ok(Account::default()));
}

#[test]
fn overspending_across_two_transactions_is_rejected() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = [2u8; 32];

    fund(&db, &alice_addr, 1_000);

    // Individually affordable, jointly not: 600 + 600 > 1000. Correct nonces,
    // so only the running balance in the overlay can catch this.
    let block = block_of(vec![
        transfer(&alice, &bob_addr, 600, 0),
        transfer(&alice, &bob_addr, 600, 1),
    ]);

    assert_eq!(
        db.apply_block(&block, BlockContext::GENESIS),
        Err(NodeError::InsufficientBalance {
            address: hex::encode(alice_addr),
            required: 600,
            available: 400,
        })
    );
    assert_eq!(db.get_account(&alice_addr).map(|a| a.balance), Ok(1_000));
}

#[test]
fn replaying_a_committed_transaction_is_rejected() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = [2u8; 32];

    fund(&db, &alice_addr, 1_000);

    let tx = transfer(&alice, &bob_addr, 300, 0);
    db.apply_block(&block_of(vec![tx.clone()]), BlockContext::GENESIS)
        .expect("first application succeeds");

    // The identical, still-valid signature must not spend twice.
    assert_eq!(
        db.apply_block(&block_of(vec![tx]), BlockContext::GENESIS),
        Err(NodeError::InvalidNonce {
            address: hex::encode(alice_addr),
            expected: 1,
            actual: 0,
        })
    );
    assert_eq!(db.get_account(&alice_addr).map(|a| a.balance), Ok(700));
}

// ---------------------------------------------------------------------------
// nonce enforcement
// ---------------------------------------------------------------------------

#[test]
fn future_nonce_is_rejected() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);

    fund(&db, &alice_addr, 1_000);

    // Gaps are not allowed: nonce must be exactly the expected value.
    assert_eq!(
        db.apply_block(
            &block_of(vec![transfer(&alice, &[2u8; 32], 10, 5)]),
            BlockContext::GENESIS
        ),
        Err(NodeError::InvalidNonce {
            address: hex::encode(alice_addr),
            expected: 0,
            actual: 5,
        })
    );
}

#[test]
fn stale_nonce_is_rejected() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);

    db.put_account(
        &alice_addr,
        &Account {
            balance: 1_000,
            nonce: 3,
        },
    )
    .expect("seed account");

    assert_eq!(
        db.apply_block(
            &block_of(vec![transfer(&alice, &[2u8; 32], 10, 2)]),
            BlockContext::GENESIS
        ),
        Err(NodeError::InvalidNonce {
            address: hex::encode(alice_addr),
            expected: 3,
            actual: 2,
        })
    );
}

// ---------------------------------------------------------------------------
// rollback / atomicity
// ---------------------------------------------------------------------------

#[test]
fn failure_late_in_a_block_rolls_back_earlier_transactions() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let bob = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let bob_addr = address_of(&bob);
    let carol_addr = [7u8; 32];

    fund(&db, &alice_addr, 1_000);
    fund(&db, &bob_addr, 50);

    let root_before = db.state_root().expect("root");

    // Two good transfers, then one that Bob cannot afford.
    let block = block_of(vec![
        transfer(&alice, &carol_addr, 100, 0),
        transfer(&alice, &carol_addr, 100, 1),
        transfer(&bob, &carol_addr, 5_000, 0),
    ]);

    assert!(matches!(
        db.apply_block(&block, BlockContext::GENESIS),
        Err(NodeError::InsufficientBalance { .. })
    ));

    // Every account is exactly as it was; the WriteBatch never reached disk.
    assert_eq!(db.get_account(&alice_addr).map(|a| a.balance), Ok(1_000));
    assert_eq!(db.get_account(&alice_addr).map(|a| a.nonce), Ok(0));
    assert_eq!(db.get_account(&bob_addr).map(|a| a.balance), Ok(50));
    assert_eq!(db.get_account(&carol_addr), Ok(Account::default()));
    assert_eq!(db.state_root(), Ok(root_before));
}

#[test]
fn unsigned_transaction_aborts_the_whole_block() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);

    fund(&db, &alice_addr, 1_000);

    let good = transfer(&alice, &[2u8; 32], 10, 0);
    let unsigned = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 1,
            recipient: [3u8; 32],
        }],
        0,
    );

    assert_eq!(
        db.apply_block(&block_of(vec![good, unsigned]), BlockContext::GENESIS),
        Err(NodeError::MissingSignature)
    );
    assert_eq!(db.get_account(&alice_addr).map(|a| a.balance), Ok(1_000));
}

#[test]
fn tampered_transaction_aborts_the_whole_block() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);

    fund(&db, &alice_addr, 1_000);

    let mut tampered = transfer(&alice, &[2u8; 32], 10, 0);
    tampered.outputs[0].amount = 999; // invalidates the signature

    assert_eq!(
        db.apply_block(&block_of(vec![tampered]), BlockContext::GENESIS),
        Err(NodeError::SignatureVerification)
    );
    assert_eq!(db.get_account(&alice_addr).map(|a| a.balance), Ok(1_000));
}

// ---------------------------------------------------------------------------
// Merkle state root
// ---------------------------------------------------------------------------

#[test]
fn empty_state_roots_to_zero() {
    let (db, _dir) = open_db();
    assert_eq!(db.state_root(), Ok([0u8; 32]));
}

#[test]
fn state_root_changes_when_state_changes() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);

    fund(&db, &alice_addr, 1_000);
    let before = db.state_root().expect("root");

    db.apply_block(
        &block_of(vec![transfer(&alice, &[2u8; 32], 100, 0)]),
        BlockContext::GENESIS,
    )
    .expect("apply");
    let after = db.state_root().expect("root");

    assert_ne!(before, after);
}

#[test]
fn state_root_is_independent_of_insertion_order() {
    // Leaves are ordered by address, so the same account set must root
    // identically regardless of the order it was written in.
    let (first, _d1) = open_db();
    fund(&first, &[1u8; 32], 10);
    fund(&first, &[2u8; 32], 20);
    fund(&first, &[3u8; 32], 30);

    let (second, _d2) = open_db();
    fund(&second, &[3u8; 32], 30);
    fund(&second, &[1u8; 32], 10);
    fund(&second, &[2u8; 32], 20);

    assert_eq!(first.state_root(), second.state_root());
}

#[test]
fn apply_block_returns_the_committed_root() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    fund(&db, &address_of(&alice), 1_000);

    let returned = db
        .apply_block(
            &block_of(vec![transfer(&alice, &[2u8; 32], 100, 0)]),
            BlockContext::GENESIS,
        )
        .expect("apply");

    assert_eq!(db.state_root(), Ok(returned));
}

#[test]
fn checked_apply_rejects_a_header_with_the_wrong_state_root() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    fund(&db, &alice_addr, 1_000);

    // Header commits an all-zero root, which execution will not produce.
    let block = block_of(vec![transfer(&alice, &[2u8; 32], 100, 0)]);

    assert!(matches!(
        db.apply_block_checked(&block, BlockContext::GENESIS),
        Err(NodeError::StateRootMismatch { .. })
    ));
    assert_eq!(db.get_account(&alice_addr).map(|a| a.balance), Ok(1_000));
}

#[test]
fn checked_apply_commits_when_the_header_root_matches() {
    let (db, _dir) = open_db();
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    fund(&db, &alice_addr, 1_000);

    let mut block = block_of(vec![transfer(&alice, &[2u8; 32], 100, 0)]);

    // Dry-run against a throwaway database holding identical state to learn the
    // root the header must commit to.
    let (mirror, _mirror_dir) = open_db();
    fund(&mirror, &alice_addr, 1_000);
    let expected = mirror
        .apply_block(&block, BlockContext::GENESIS)
        .expect("mirror apply");

    block.header.state_root = expected;
    assert_eq!(
        db.apply_block_checked(&block, BlockContext::GENESIS),
        Ok(expected)
    );
    assert_eq!(db.get_account(&alice_addr).map(|a| a.balance), Ok(900));
}
