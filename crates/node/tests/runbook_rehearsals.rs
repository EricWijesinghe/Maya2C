//! Runbook rehearsals (Master Prompt 19 §3): each failure a runbook in
//! `docs/runbooks/` handles is caused here on purpose, and the runbook's
//! recovery is carried out and checked. Wall-clock timings are printed so the
//! report can quote them.
//!
//! | runbook | what is rehearsed |
//! |---|---|
//! | mempool-full | a full pool refuses, a block drains it, admission resumes |
//! | deep-reorg | a 20-block reorganisation onto a heavier branch, state rewritten |
//! | state-root-mismatch | a block whose declared root is wrong is refused; the tip holds |
//! | db-corruption | a silently altered account is detected by root check, and the state is rebuilt by replaying the node's own block store into a fresh database |

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Instant;

use custom_l1_node::consensus::chain::{Chain, ChainConfig, InsertOutcome};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::network::Mempool;
use custom_l1_node::state::account::Account;
use custom_l1_node::state::db::StateDB;
use tempfile::TempDir;

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

fn key(i: u8) -> HybridSigningKey {
    signing_key_from_seed(&[i; 32]).unwrap()
}

fn transfer(from: &HybridSigningKey, amount: u64, nonce: u64) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount,
            recipient: [0x77; 32],
        }],
        nonce,
    );
    tx.sign(from).unwrap();
    tx
}

/// A chain whose state funds keys `0..n`.
fn funded(n: u8) -> (Chain, TempDir) {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    for i in 0..n {
        state
            .put_account(
                &key(i).address(),
                &Account {
                    balance: 1_000_000,
                    nonce: 0,
                },
            )
            .unwrap();
    }
    let chain = Chain::open(state, genesis(), ChainConfig::without_pow_verification()).unwrap();
    (chain, dir)
}

fn extend(c: &mut Chain, stamp: u64, txs: Vec<Transaction>) -> Block {
    let block = c.candidate_block(stamp, txs).unwrap();
    c.insert_block(block.clone()).unwrap();
    block
}

#[test]
fn mempool_full_refuses_then_drains_then_admits() {
    let (mut c, _d) = funded(6);
    let pool = Mempool::with_capacity(Arc::clone(c.state()), 5);
    for i in 0..5 {
        assert!(pool.insert(transfer(&key(i), 1, 0)).unwrap());
    }
    let err = pool.insert(transfer(&key(5), 1, 0)).unwrap_err();
    assert!(
        matches!(err, NodeError::MempoolRejected(ref m) if m.contains("full")),
        "{err:?}"
    );
    // Runbook step: let a block drain the pool.
    let started = Instant::now();
    let pending = pool.snapshot();
    let block = extend(&mut c, 1_000_010, pending);
    let ids: Vec<[u8; 32]> = block.transactions.iter().map(Transaction::txid).collect();
    pool.remove_all(&ids);
    assert!(pool.is_empty());
    assert!(
        pool.insert(transfer(&key(5), 1, 0)).unwrap(),
        "admission resumed"
    );
    println!(
        "mempool-full: 5/5 full, 6th refused; one block drained {} transactions and admission resumed in {:.1} ms",
        ids.len(),
        started.elapsed().as_secs_f64() * 1e3
    );
}

#[test]
fn deep_reorg_of_twenty_blocks_rewrites_state_to_the_heavier_branch() {
    let (mut main, _a) = funded(1);
    let (mut shadow, _b) = funded(1);
    // Branch A on the main node: 20 blocks, key 0 spending 10 each.
    for n in 0..20u64 {
        extend(
            &mut main,
            1_000_010 + n * 15,
            vec![transfer(&key(0), 10, n)],
        );
    }
    // Branch B on a competing node: 21 blocks spending 7 each, from genesis.
    let mut branch = Vec::new();
    for n in 0..21u64 {
        branch.push(extend(
            &mut shadow,
            1_000_011 + n * 15,
            vec![transfer(&key(0), 7, n)],
        ));
    }
    let started = Instant::now();
    let mut reverted = 0;
    for block in branch {
        if let InsertOutcome::Reorganized { reverted: r, .. } = main.insert_block(block).unwrap() {
            reverted = r.len();
        }
    }
    let secs = started.elapsed().as_secs_f64();
    assert_eq!(reverted, 20, "the whole of branch A is reverted");
    assert_eq!(main.tip(), shadow.tip());
    assert_eq!(
        main.state().state_root().unwrap(),
        shadow.state().state_root().unwrap()
    );
    assert_eq!(
        main.state().get_account(&key(0).address()).unwrap().balance,
        1_000_000 - 21 * 7
    );
    println!(
        "deep-reorg: 20 blocks reverted, 21 applied, state identical to the heavier branch's, in {secs:.2} s"
    );
}

#[test]
fn a_block_declaring_the_wrong_state_root_is_refused_and_the_tip_holds() {
    let (mut c, _d) = funded(1);
    extend(&mut c, 1_000_010, vec![]);
    let tip = c.tip();
    let mut bad = c
        .candidate_block(1_000_020, vec![transfer(&key(0), 5, 0)])
        .unwrap();
    bad.header.state_root[0] ^= 1;
    let err = c.insert_block(bad).unwrap_err();
    assert!(
        matches!(err, NodeError::StateRootMismatch { .. }),
        "{err:?}"
    );
    assert_eq!(c.tip(), tip, "the tip did not move");
    // Recovery: the honest block for the same height lands.
    extend(&mut c, 1_000_020, vec![transfer(&key(0), 5, 0)]);
    println!(
        "state-root-mismatch: refused with StateRootMismatch, tip held, next honest block accepted"
    );
}

#[test]
fn silent_state_corruption_is_detected_and_rebuilt_from_the_block_store() {
    let dir = TempDir::new().unwrap();
    let blocks: Vec<Block>;
    {
        let state = Arc::new(StateDB::open(dir.path()).unwrap());
        state
            .put_account(
                &key(0).address(),
                &Account {
                    balance: 1_000_000,
                    nonce: 0,
                },
            )
            .unwrap();
        let mut c = Chain::open(state, genesis(), ChainConfig::without_pow_verification()).unwrap();
        blocks = (0..10u64)
            .map(|n| extend(&mut c, 1_000_010 + n, vec![transfer(&key(0), 3, n)]))
            .collect();
        // The corruption: a bit rot or a bad restore changes one balance.
        c.state()
            .put_account(
                &key(0).address(),
                &Account {
                    balance: 42,
                    nonce: 10,
                },
            )
            .unwrap();
    }
    // Detection: the node refuses to open a database whose state root is not
    // the one its tip header committed to. Nothing to notice by hand.
    let started = Instant::now();
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    let refused = Chain::open(
        Arc::clone(&state),
        genesis(),
        ChainConfig::without_pow_verification(),
    );
    let Err(NodeError::StateRootMismatch { expected, .. }) = refused else {
        panic!(
            "a corrupted database opened: {:?}",
            refused.map(|c| c.height())
        );
    };
    let tip_root: [u8; 32] = hex::decode(&expected).unwrap().try_into().unwrap();
    // Recovery: a fresh database, the genesis allocation, and the node's own
    // blocks replayed — each re-executed and root-checked on the way in.
    let fresh_dir = TempDir::new().unwrap();
    let fresh = Arc::new(StateDB::open(fresh_dir.path()).unwrap());
    fresh
        .put_account(
            &key(0).address(),
            &Account {
                balance: 1_000_000,
                nonce: 0,
            },
        )
        .unwrap();
    let mut rebuilt = Chain::open(
        Arc::clone(&fresh),
        genesis(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap();
    for b in blocks {
        rebuilt.insert_block(b).unwrap();
    }
    assert_eq!(fresh.state_root().unwrap(), tip_root);
    println!(
        "db-corruption: the node refused to open (StateRootMismatch); rebuilt by replaying 10 stored blocks in {:.1} ms; root matches the tip",
        started.elapsed().as_secs_f64() * 1e3
    );
}
