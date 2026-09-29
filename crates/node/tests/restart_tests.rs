//! Restarting a node: it comes back on the chain it had, over the state it had.
//!
//! Before the block store existed this was impossible. No block was persisted,
//! so a restarted node rebuilt its chain from genesis alone. Worse,
//! `seed_state` rewrote the genesis allocations over the evolved state, the
//! genesis root check then failed, and the node refused to start. These tests
//! pin the fix: a node restarts on its own tip, nothing is re-seeded, and a
//! crash between batches cannot leave state and tip disagreeing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig, InsertOutcome};
use custom_l1_node::consensus::difficulty::work_from_target;
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, generate_signing_key};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::account::{Account, Address};
use custom_l1_node::state::db::StateDB;
use tempfile::TempDir;

fn genesis(timestamp: u64) -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

fn open_state(dir: &Path) -> Arc<StateDB> {
    Arc::new(StateDB::open(dir).expect("open state"))
}

fn open_chain(state: Arc<StateDB>) -> Result<Chain, NodeError> {
    Chain::open(
        state,
        genesis(1_000_000),
        ChainConfig::without_pow_verification(),
    )
}

/// A fresh database funding `key`, and the chain opened on it.
fn funded(dir: &Path, key: &HybridSigningKey) -> Chain {
    let state = open_state(dir);
    state
        .put_account(
            &key.address(),
            &Account {
                balance: 1_000_000,
                nonce: 0,
            },
        )
        .expect("fund");
    open_chain(state).expect("open chain")
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

/// Mines `count` blocks on `chain`'s tip, each carrying one transfer from
/// `key` starting at `first_nonce`, and returns their ids.
fn mine(chain: &mut Chain, key: &HybridSigningKey, first_nonce: u64, count: u64) -> Vec<[u8; 32]> {
    (0..count)
        .map(|offset| {
            let timestamp = 1_000_000 + (chain.height() + 1) * 15;
            let tx = transfer(key, [0x77; 32], 10, first_nonce + offset);
            let block = chain
                .candidate_block(timestamp, vec![tx])
                .expect("candidate");
            let id = block.header.id();
            assert!(matches!(
                chain.insert_block(block).expect("insert"),
                InsertOutcome::Extended { .. }
            ));
            id
        })
        .collect()
}

#[test]
fn a_node_restarts_on_the_tip_and_the_state_it_had() {
    let dir = TempDir::new().expect("dir");
    let key = generate_signing_key().expect("key");

    let (tip, height, root, balance) = {
        let mut chain = funded(dir.path(), &key);
        mine(&mut chain, &key, 0, 5);
        let state = chain.state();
        (
            chain.tip(),
            chain.height(),
            state.state_root().expect("root"),
            state.get_account(&key.address()).expect("account").balance,
        )
    };

    // Reopened without re-funding: the allocations must not be applied again.
    let mut chain = open_chain(open_state(dir.path())).expect("reopen");
    assert_eq!(chain.tip(), tip);
    assert_eq!(chain.height(), height);
    assert_eq!(chain.state().state_root().expect("root"), root);
    assert_eq!(
        chain
            .state()
            .get_account(&key.address())
            .expect("account")
            .balance,
        balance
    );

    // And it goes on extending the same chain.
    mine(&mut chain, &key, 5, 1);
    assert_eq!(chain.height(), height + 1);
}

#[test]
fn a_database_started_with_another_genesis_is_refused() {
    let dir = TempDir::new().expect("dir");
    let key = generate_signing_key().expect("key");
    drop(funded(dir.path(), &key));

    let reopened = Chain::open(
        open_state(dir.path()),
        genesis(2_000_000),
        ChainConfig::without_pow_verification(),
    );
    assert!(matches!(reopened, Err(NodeError::GenesisMismatch { .. })));
}

#[test]
fn side_branches_survive_a_restart_and_can_still_win() {
    let dir = TempDir::new().expect("dir");
    let shadow_dir = TempDir::new().expect("dir");
    let key = generate_signing_key().expect("key");

    // The main chain: two blocks. A competing miner: two blocks of its own
    // from the same genesis state, then a third.
    let mut shadow = funded(shadow_dir.path(), &key);
    let branch = {
        let ids = mine(&mut shadow, &key, 0, 3);
        ids.iter()
            .map(|id| shadow.state().load_block(id).expect("load").expect("body"))
            .collect::<Vec<Block>>()
    };

    {
        let mut chain = funded(dir.path(), &key);
        // Different transfers from the shadow's, so different blocks.
        for nonce in 0..2 {
            let tx = transfer(&key, [0x55; 32], 1, nonce);
            let block = chain
                .candidate_block(1_000_000 + (nonce + 1) * 15 + 1, vec![tx])
                .expect("candidate");
            chain.insert_block(block).expect("main");
        }
        for block in &branch[..2] {
            assert!(matches!(
                chain.insert_block(block.clone()).expect("side"),
                InsertOutcome::SideBranch { .. }
            ));
        }
    }

    let mut chain = open_chain(open_state(dir.path())).expect("reopen");
    assert!(
        chain.contains(&branch[1].header.id()),
        "the side branch was forgotten"
    );

    // Its third block outweighs the main chain. The reorg needs the side
    // branch's bodies, which only the block store kept across the restart.
    assert!(matches!(
        chain.insert_block(branch[2].clone()).expect("reorg"),
        InsertOutcome::Reorganized { .. }
    ));
    assert_eq!(chain.tip(), branch[2].header.id());
    assert_eq!(
        chain.state().state_root().expect("root"),
        branch[2].header.state_root
    );
}

#[test]

mod common;
fn a_heavier_branch_stored_before_a_crash_is_adopted_on_open() {
    // A crash between storing a block and applying it leaves a heavier branch
    // on disk that the tip never moved to. Opening finishes the job.
    let dir = TempDir::new().expect("dir");
    let shadow_dir = TempDir::new().expect("dir");
    let key = generate_signing_key().expect("key");

    let mut shadow = funded(shadow_dir.path(), &key);
    let ids = mine(&mut shadow, &key, 0, 2);

    {
        let chain = funded(dir.path(), &key);
        let mut total = work_from_target(&genesis(1_000_000).header.difficulty_target);
        for (index, id) in ids.iter().enumerate() {
            let block = shadow.state().load_block(id).expect("load").expect("body");
            total = total.saturating_add(work_from_target(&block.header.difficulty_target));
            chain
                .state()
                .store_block(&block, index as u64 + 1, total)
                .expect("store");
        }
    }

    let chain = open_chain(open_state(dir.path())).expect("reopen");
    assert_eq!(chain.tip(), ids[1]);
    assert_eq!(
        chain.state().state_root().expect("root"),
        shadow.state().state_root().expect("root")
    );
}
