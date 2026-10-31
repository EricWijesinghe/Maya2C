//! The block byte cap (ADR-072), executed by the node's own selection and
//! import code.
//!
//! What these pin:
//!
//! - an executable order over the cap is cut at a prefix, and the block that
//!   comes out is within the cap by the encoder's own count;
//! - a transaction the cut leaves out is not lost: the next block carries it,
//!   and it executes once, however many times it is proposed again;
//! - two nodes that select from the same order reach the same block and root;
//! - a block whose wire form is over the cap is refused on import.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::block::{Block, BlockHeader, admitted_prefix, block_wire_len};
use custom_l1_node::core::transaction::{MAX_TX_BYTES, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::{Account, Address, BlockContext, StateDB};
use maya_fee_market::limits::MAX_BLOCK_BYTES;
use primitive_types::U256;
use tempfile::TempDir;

mod common;

const BOB: Address = [0xB0; 32];
const FUNDS: u64 = 1_000_000;
const PAYMENT: u64 = 10;

/// A state database bound to the test chain and funded for the given senders.
struct Ledger {
    db: StateDB,
    _dir: TempDir,
}

impl Ledger {
    fn funded(senders: &[Address]) -> Self {
        let dir = TempDir::new().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        common::bind(&db);
        for a in senders {
            db.put_account(
                a,
                &Account {
                    balance: U256::from(FUNDS),
                    nonce: 0,
                },
            )
            .unwrap();
        }
        Self { db, _dir: dir }
    }

    /// Applies `txs` as the block at `height`, returning the state root.
    fn apply(&self, height: u64, txs: Vec<Transaction>) -> [u8; 32] {
        let header = BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_790_000_000 + height,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        };
        self.db
            .apply_block(&Block::new(header, txs), BlockContext::at_height(height))
            .unwrap()
    }

    fn balance(&self, a: &Address) -> U256 {
        self.db.get_account(a).unwrap().balance
    }
}

fn key() -> HybridSigningKey {
    generate_signing_key().unwrap()
}

fn transfer(key: &HybridSigningKey, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: U256::from(PAYMENT),
            recipient: BOB,
        }],
        nonce,
    );
    tx.sign(key, &common::test_chain()).unwrap();
    tx
}

fn ids(txs: &[Transaction]) -> Vec<[u8; 32]> {
    txs.iter().map(Transaction::txid).collect()
}

fn lens(txs: &[Transaction]) -> Vec<usize> {
    txs.iter().map(|t| t.to_bytes().len()).collect()
}

fn target() -> [u8; 32] {
    target_from_leading_zero_bits(0)
}

fn context(height: u64) -> BlockContext {
    BlockContext::at_height(height)
}

#[test]
fn an_order_over_the_cap_is_cut_at_a_prefix_that_fits() {
    let user = key();
    let ledger = Ledger::funded(&[user.address()]);
    let txs: Vec<Transaction> = (0..5).map(|n| transfer(&user, n)).collect();
    let encoded = lens(&txs);
    let cap = block_wire_len(encoded[..2].iter().copied());

    let kept = ledger
        .db
        .select_applicable_within(txs.clone(), context(1), target(), cap)
        .unwrap();

    assert_eq!(
        ids(&kept),
        ids(&txs[..2]),
        "the first two fit, the rest wait"
    );
    assert_eq!(admitted_prefix(&encoded, cap), 2);
    assert_eq!(
        admitted_prefix(&encoded, cap - 1),
        1,
        "one byte less takes one"
    );
    let block = Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1,
            nonce: 0,
            difficulty_target: target(),
            tx_root: [0; 32],
        },
        kept.clone(),
    );
    assert!(
        block.to_bytes().len() as u64 <= cap,
        "the block is within the cap"
    );
    assert_eq!(block.to_bytes().len() as u64, block_wire_len(lens(&kept)));
}

#[test]
fn a_transaction_left_out_is_applied_once_in_a_later_block() {
    let user = key();
    let ledger = Ledger::funded(&[user.address()]);
    let txs: Vec<Transaction> = (0..3).map(|n| transfer(&user, n)).collect();
    let cap = block_wire_len(lens(&txs[..2]));

    let first = ledger
        .db
        .select_applicable_within(txs.clone(), context(1), target(), cap)
        .unwrap();
    assert_eq!(ids(&first), ids(&txs[..2]));
    ledger.apply(1, first);

    // The proposer proposes what was left, and then the whole queue again.
    let second = ledger
        .db
        .select_applicable_within(txs[2..].to_vec(), context(2), target(), cap)
        .unwrap();
    assert_eq!(ids(&second), ids(&txs[2..]), "the left-over one is carried");
    ledger.apply(2, second);

    let replay = ledger
        .db
        .select_applicable(txs.clone(), context(3), target())
        .unwrap();
    assert!(
        replay.is_empty(),
        "a proposed-again transaction does not execute twice"
    );
    assert_eq!(ledger.balance(&BOB), U256::from(PAYMENT * 3));
}

#[test]
fn two_nodes_selecting_from_the_same_order_reach_the_same_block_and_root() {
    let user = key();
    let txs: Vec<Transaction> = (0..4).map(|n| transfer(&user, n)).collect();
    let cap = block_wire_len(lens(&txs[..3]));
    let a = Ledger::funded(&[user.address()]);
    let b = Ledger::funded(&[user.address()]);

    let kept_a =
        a.db.select_applicable_within(txs.clone(), context(1), target(), cap)
            .unwrap();
    let kept_b =
        b.db.select_applicable_within(txs.clone(), context(1), target(), cap)
            .unwrap();

    assert_eq!(ids(&kept_a), ids(&kept_b));
    assert_eq!(
        a.apply(1, kept_a),
        b.apply(1, kept_b),
        "the same state root"
    );
}

#[test]
fn a_block_over_the_cap_is_refused_on_import() {
    let dir = TempDir::new().unwrap();
    // Unbound: `Chain::open` binds the database to the genesis it is given.
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    let genesis = Block::new(
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
    );
    let mut chain: Chain = common::open_chain(state, genesis, ChainConfig::dag_bft()).unwrap();
    let tx = transfer(&key(), 0);
    // Repeats of one transaction are enough: the size rule is checked before
    // any transaction is executed, so the duplicates need not be valid.
    let count = usize::try_from(MAX_BLOCK_BYTES).unwrap() / tx.to_bytes().len() + 1;
    let block = Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_000_001,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        vec![tx; count],
    );

    match chain.insert_block(block) {
        Err(NodeError::BlockTooLarge { bytes, cap }) => {
            assert_eq!(cap, MAX_BLOCK_BYTES);
            assert!(bytes > cap);
        }
        other => panic!("an over-cap block must be refused, got {other:?}"),
    }
}

#[test]
fn the_wire_count_is_the_encoders_and_a_maximal_transaction_fits_alone() {
    let user = key();
    let tx = transfer(&user, 0);
    let block = Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1,
            nonce: 0,
            difficulty_target: target(),
            tx_root: [0; 32],
        },
        vec![tx.clone()],
    );
    assert_eq!(
        block.to_bytes().len() as u64,
        block_wire_len([tx.to_bytes().len()])
    );
    assert_eq!(admitted_prefix(&[MAX_TX_BYTES], MAX_BLOCK_BYTES), 1);
    assert_eq!(
        admitted_prefix(&[MAX_TX_BYTES, MAX_TX_BYTES], MAX_BLOCK_BYTES),
        1
    );
}
