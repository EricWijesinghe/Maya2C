//! Pooled entries that can never execute are refused at admission or pruned
//! once a round drops them, so the BFT feed does not re-propose them every
//! round without a fee (independent audit finding CT-01).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use primitive_types::U256;
use std::sync::Arc;

use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use custom_l1_node::error::NodeError;
use custom_l1_node::network::mempool::Mempool;
use custom_l1_node::state::{Account, StateDB};
use tempfile::TempDir;

mod common;

const SEED: [u8; 32] = [42; 32];

fn funded(dir: &TempDir) -> Arc<StateDB> {
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    common::bind(&state);
    let sender = signing_key_from_seed(&SEED).unwrap().address();
    let account = Account {
        balance: U256::from(1_000),
        nonce: 0,
    };
    state.put_account(&sender, &account).unwrap();
    state
}

fn pay(nonce: u64, amount: u64) -> Transaction {
    let output = TxOutput {
        amount: U256::from(amount),
        recipient: [7; 32],
    };
    let mut tx = Transaction::new(vec![], vec![output], nonce);
    tx.sign(
        &signing_key_from_seed(&SEED).unwrap(),
        &common::test_chain(),
    )
    .unwrap();
    tx
}

#[test]
fn a_nonce_past_the_pooled_run_is_refused_until_the_gap_fills() {
    let dir = TempDir::new().unwrap();
    let pool = Mempool::new(funded(&dir));

    assert!(pool.insert(pay(0, 1)).unwrap());
    let err = pool.insert(pay(2, 1)).unwrap_err();
    assert!(matches!(err, NodeError::MempoolRejected(_)), "{err:?}");

    assert!(pool.insert(pay(1, 1)).unwrap());
    assert!(pool.insert(pay(2, 1)).unwrap(), "the gap is filled now");
}

#[test]
fn several_entries_at_the_next_nonce_are_not_a_gap() {
    // Competing spends of one nonce are legal to pool: one executes, and the
    // rest go stale and are pruned (below).
    let dir = TempDir::new().unwrap();
    let pool = Mempool::new(funded(&dir));
    assert!(pool.insert(pay(0, 1)).unwrap());
    assert!(pool.insert(pay(0, 2)).unwrap());
    assert!(pool.insert(pay(1, 1)).unwrap());
}

#[test]
fn a_dropped_entry_made_stale_by_a_commit_is_pruned() {
    let dir = TempDir::new().unwrap();
    let state = funded(&dir);
    let pool = Mempool::new(Arc::clone(&state));
    let pooled = pay(0, 1);
    let still_good = pay(1, 1);
    assert!(pool.insert(pooled.clone()).unwrap());
    assert!(pool.insert(still_good.clone()).unwrap());

    // A different transaction with nonce 0 committed: `pooled` can never run.
    let sender = signing_key_from_seed(&SEED).unwrap().address();
    let committed = Account {
        balance: U256::from(990),
        nonce: 1,
    };
    state.put_account(&sender, &committed).unwrap();

    let dropped = [pooled.txid(), still_good.txid()];
    assert_eq!(pool.prune_invalid(&dropped), 1);
    assert!(!pool.contains(&pooled.txid()));
    assert!(pool.contains(&still_good.txid()), "nonce 1 still executes");
}

/// The head of a pooled run can leave the pool without executing (a fee
/// eviction, a refusal). Everything behind it is then a gap that a count of
/// pooled entries would not notice; checking each nonce does.
#[test]
fn a_run_whose_head_left_the_pool_is_pruned_whole() {
    let dir = TempDir::new().unwrap();
    let pool = Mempool::new(funded(&dir));
    let run: Vec<Transaction> = (0..3).map(|nonce| pay(nonce, 1)).collect();
    for tx in &run {
        assert!(pool.insert(tx.clone()).unwrap());
    }
    assert_eq!(pool.remove_all(&[run[0].txid()]), 1);

    // Out of order on purpose: the pass sorts by nonce itself.
    let dropped = [run[2].txid(), run[1].txid()];
    assert_eq!(pool.prune_invalid(&dropped), 2);
    assert!(pool.is_empty());
}

/// An out-of-order round is normal: the DAG may order nonce 1 before 0. The
/// entry that ran first is gone, and the one behind it must stay.
#[test]
fn a_dropped_entry_that_can_still_run_is_kept() {
    let dir = TempDir::new().unwrap();
    let pool = Mempool::new(funded(&dir));
    let (first, second) = (pay(0, 1), pay(1, 1));
    assert!(pool.insert(first.clone()).unwrap());
    assert!(pool.insert(second.clone()).unwrap());

    assert_eq!(pool.prune_invalid(&[second.txid()]), 0);
    assert!(pool.contains(&second.txid()));
}
