//! Upgrade rehearsal (Master Prompt 15 §4): 100 nodes, 70 upgraded before the
//! activation height H and 30 late.
//!
//! Every node is a real `Chain` over its own `RocksDB` state. "Upgraded" is a
//! binary that implements the version scheduled at H — its
//! `ChainConfig::unsupported_upgrade` is `None`; a late node's binary does not,
//! so it carries the scheduled upgrade as its first unsupported one. That is
//! exactly what `UpgradeSchedule::first_unsupported` computes for the two
//! binaries. The rules of the new version are the old ones here; what is
//! rehearsed is the mechanism: the network keeps producing, a late node halts
//! at H with "upgrade required before height H" and a consistent state, and
//! after upgrading it catches up to the same tip.
//!
//! Lives with the node rather than in `sim/` because it drives the node's own
//! chain and storage; `maya-sim` is dependency-free by design (ADR-006).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::error::NodeError;
use custom_l1_node::state::db::StateDB;
use custom_l1_node::upgrade::{ProtocolUpgrade, SUPPORTED_PROTOCOL_VERSION, UpgradeSchedule};
use tempfile::TempDir;

const NODES: usize = 100;
const UPGRADED: usize = 70;
const H: u64 = 20;
const HEAD: u64 = 40;

fn genesis() -> Block {
    let header = BlockHeader {
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp: 1_000_000,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    Block::new(header, Vec::new())
}

fn config(upgraded: bool) -> ChainConfig {
    let schedule = UpgradeSchedule::new(vec![ProtocolUpgrade {
        version: SUPPORTED_PROTOCOL_VERSION + 1,
        activation_height: H,
    }])
    .unwrap();
    let base = ChainConfig::without_pow_verification();
    if upgraded {
        base
    } else {
        base.with_upgrades(&schedule)
    }
}

fn open(dir: &TempDir, upgraded: bool) -> Chain {
    Chain::open(
        Arc::new(StateDB::open(dir.path()).unwrap()),
        genesis(),
        config(upgraded),
    )
    .unwrap()
}

#[test]
fn late_nodes_halt_at_h_and_catch_up_after_upgrading() {
    // The network's blocks, produced by an upgraded node.
    let producer_dir = TempDir::new().unwrap();
    let mut producer = open(&producer_dir, true);
    let mut blocks = Vec::new();
    for h in 1..=HEAD {
        let block = producer
            .candidate_block(1_000_000 + h * 15, Vec::new())
            .unwrap();
        producer.insert_block(block.clone()).unwrap();
        blocks.push(block);
    }

    let dirs: Vec<TempDir> = (0..NODES).map(|_| TempDir::new().unwrap()).collect();
    let mut halted = 0;
    for (i, dir) in dirs.iter().enumerate() {
        let upgraded = i < UPGRADED;
        let mut chain = open(dir, upgraded);
        for block in &blocks {
            match chain.insert_block(block.clone()) {
                Ok(_) => {}
                Err(NodeError::UpgradeRequired { height, .. }) if !upgraded => {
                    assert_eq!(height, H);
                    halted += 1;
                    break;
                }
                Err(e) => panic!("node {i}: {e}"),
            }
        }
        if upgraded {
            assert_eq!(
                (chain.height(), chain.tip()),
                (HEAD, producer.tip()),
                "upgraded node {i} left the network"
            );
        } else {
            // Halted cleanly: last good block is H-1 and its state root is what the header committed.
            assert_eq!(chain.height(), H - 1, "late node {i} did not stop at H");
            let tip = chain.get(&chain.tip()).unwrap();
            assert_eq!(chain.state().state_root().unwrap(), tip.header.state_root);
        }
    }
    assert_eq!(halted, NODES - UPGRADED);

    // The late operators upgrade: same data directory, new binary.
    for dir in &dirs[UPGRADED..] {
        let mut chain = open(dir, true);
        assert_eq!(chain.height(), H - 1, "restart lost blocks");
        for block in &blocks[usize::try_from(H - 1).unwrap()..] {
            chain.insert_block(block.clone()).unwrap();
        }
        assert_eq!((chain.height(), chain.tip()), (HEAD, producer.tip()));
    }
    println!(
        "{NODES} nodes: {UPGRADED} upgraded followed to height {HEAD}; {halted} late halted at {H} with UpgradeRequired, then caught up after upgrading"
    );
}

#[test]
fn the_halt_message_names_the_height() {
    let dir = TempDir::new().unwrap();
    let mut late = open(&dir, false);
    let producer_dir = TempDir::new().unwrap();
    let mut producer = open(&producer_dir, true);
    for h in 1..=H {
        let block = producer
            .candidate_block(1_000_000 + h * 15, Vec::new())
            .unwrap();
        producer.insert_block(block.clone()).unwrap();
        if let Err(e) = late.insert_block(block) {
            assert!(
                e.to_string()
                    .starts_with(&format!("upgrade required before height {H}")),
                "{e}"
            );
            return;
        }
    }
    panic!("the late node never halted");
}
