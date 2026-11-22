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

/// A 2-of-2 ML-DSA-87 multisig (v8) spend at `nonce`. v8 frames are never
/// served from the verified-signature cache, so every check re-verifies them.
fn multisig_pay(nonce: u64) -> (Transaction, [u8; 32]) {
    use custom_l1_node::core::multisig_tx::{MultisigAuth, multisig_address};
    use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
    use maya_crypto_pq::suite::{MasterSeed, MlDsa87, SignatureSuite};

    let keys: Vec<_> = (1..=2u8)
        .map(|i| MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([i; 32])))
        .collect();
    let listed = keys
        .iter()
        .map(|k| PolicyKey {
            suite: MlDsa87::ID,
            public_key: MlDsa87::public_key(k),
        })
        .collect();
    let policy = MultisigPolicy::new(2, listed).unwrap();
    let output = TxOutput {
        amount: U256::from(1),
        recipient: [7; 32],
    };
    let mut tx = Transaction::new(vec![], vec![output], nonce);
    tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy.clone())));
    let message = tx.signing_bytes(&common::test_chain());
    let approvals = (0..2u8)
        .map(|i| Approval {
            index: i,
            signature: MlDsa87::sign(&keys[usize::from(i)], &message).unwrap(),
        })
        .collect();
    tx.multisig = Some(Box::new(
        MultisigAuth::new(policy.clone(), approvals).unwrap(),
    ));
    (tx, multisig_address(&policy))
}

/// The prune runs on the BFT consensus task every round. A signature verified
/// at admission cannot go bad, so re-verifying dropped v8 frames there (up to
/// sixteen SLH-DSA approvals each) would only stall rounds (review of #653).
#[test]
fn pruning_dropped_entries_does_not_verify_signatures_again() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    common::bind(&state);
    let (tx, account) = multisig_pay(0);
    let funds = Account {
        balance: U256::from(1_000),
        nonce: 0,
    };
    state.put_account(&account, &funds).unwrap();
    let pool = Mempool::new(Arc::clone(&state));
    assert!(pool.insert(tx.clone()).unwrap());

    let before = state.signature_verifications();
    assert_eq!(pool.prune_invalid(&[tx.txid()]), 0, "it can still run");
    assert_eq!(
        state.signature_verifications(),
        before,
        "the prune re-verified a signature"
    );
    assert!(pool.contains(&tx.txid()));
}

/// The round's bookkeeping in one call, as the BFT loop uses it: the included
/// entries leave, and a dropped entry the commit made stale goes too (CT-01).
/// A behaviour test, not an order test: the driver reports `included` after
/// the block committed, so either call order gives this result (review of #653).
#[test]
fn settling_a_round_removes_included_and_prunes_dropped() {
    let dir = TempDir::new().unwrap();
    let state = funded(&dir);
    let pool = Mempool::new(Arc::clone(&state));
    let (spent, stale, next) = (pay(0, 1), pay(0, 2), pay(1, 1));
    for tx in [&spent, &stale, &next] {
        assert!(pool.insert(tx.clone()).unwrap());
    }
    // `spent` executed: the account's nonce moved past `stale`.
    let sender = signing_key_from_seed(&SEED).unwrap().address();
    let committed = Account {
        balance: U256::from(989),
        nonce: 1,
    };
    state.put_account(&sender, &committed).unwrap();

    let pruned = pool.settle_round(&[spent.txid()], &[stale.txid(), next.txid()]);
    assert_eq!(pruned, 1);
    assert!(!pool.contains(&spent.txid()), "included entries leave");
    assert!(
        !pool.contains(&stale.txid()),
        "a stale dropped entry is pruned"
    );
    assert!(pool.contains(&next.txid()), "nonce 1 still executes");
}

/// Two pools on one node, as `main` links the gossip and RPC pools: a wallet
/// that sent nonce 0 to one node and nonce 1 to another has them in different
/// pools here, and nonce 1 is no gap (review of #653).
fn linked_pools(dir: &TempDir) -> (Mempool, Mempool) {
    let state = funded(dir);
    let (gossip, rpc) = (Mempool::new(Arc::clone(&state)), Mempool::new(state));
    assert!(rpc.link_sibling(&gossip));
    (gossip, rpc)
}

#[test]
fn a_nonce_whose_predecessor_is_in_the_sibling_pool_is_admitted() {
    let dir = TempDir::new().unwrap();
    let (gossip, rpc) = linked_pools(&dir);
    assert!(gossip.insert(pay(0, 1)).unwrap());
    assert!(
        rpc.insert(pay(1, 1)).unwrap(),
        "nonce 0 is pooled next door"
    );
    // And the other way round.
    assert!(gossip.insert(pay(2, 1)).unwrap());
    // A real gap is still a gap across both.
    let err = rpc.insert(pay(4, 1)).unwrap_err();
    assert!(matches!(err, NodeError::MempoolRejected(_)), "{err:?}");
}

#[test]
fn a_dropped_entry_whose_predecessor_is_in_the_sibling_pool_is_kept() {
    let dir = TempDir::new().unwrap();
    let (gossip, rpc) = linked_pools(&dir);
    let (first, second) = (pay(0, 1), pay(1, 1));
    assert!(rpc.insert(first.clone()).unwrap());
    assert!(gossip.insert(second.clone()).unwrap());

    assert_eq!(gossip.settle_round(&[], &[second.txid()]), 0);
    assert!(
        gossip.contains(&second.txid()),
        "nonce 0 is pooled next door"
    );

    // Once the predecessor leaves without executing, the run lost its head.
    assert_eq!(rpc.remove_all(&[first.txid()]), 1);
    assert_eq!(gossip.settle_round(&[], &[second.txid()]), 1);
}

#[test]
fn a_pool_links_to_one_sibling_only() {
    let dir = TempDir::new().unwrap();
    let (gossip, rpc) = linked_pools(&dir);
    let other_dir = TempDir::new().unwrap();
    let third = Mempool::new(funded(&other_dir));
    assert!(!third.link_sibling(&gossip), "gossip is already linked");
    assert!(!rpc.link_sibling(&third));
}
