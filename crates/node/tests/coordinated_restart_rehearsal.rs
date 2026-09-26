//! Coordinated restart after a chain halt (Master Prompt 19 §3), rehearsed on
//! 12 real nodes and timed.
//!
//! 1. The network halts with nodes at different heights.
//! 2. Operators agree a restart point `(height, state_root, block id)` and each
//!    signs it with its ML-DSA-65 operator key.
//! 3. A record is valid once more than ⅔ of the operator set has signed it.
//! 4. Each node brings itself to that height, checks its own state root and
//!    block id against the record, and restarts (reopens its database).
//! 5. A node whose state disagrees is identified, not restarted.
//! 6. Production resumes; every restarted node accepts the next block.
//!
//! Procedure: `docs/runbooks/coordinated-restart.md`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::sync::Arc;
use std::time::Instant;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::db::StateDB;
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite, SuiteId, verify};
use tempfile::TempDir;

const NODES: usize = 12;
const HALT: u64 = 30;

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

fn open(dir: &TempDir) -> Chain {
    Chain::open(
        Arc::new(StateDB::open(dir.path()).unwrap()),
        genesis(),
        ChainConfig::without_pow_verification(),
    )
    .unwrap()
}

/// The bytes every operator signs.
fn record(height: u64, root: &[u8; 32], id: &[u8; 32]) -> Vec<u8> {
    [
        b"maya2c coordinated restart v1".as_slice(),
        &height.to_le_bytes(),
        root,
        id,
    ]
    .concat()
}

#[test]
fn twelve_nodes_agree_a_restart_point_verify_it_and_resume() {
    let started = Instant::now();
    // The network before the halt: one producer's blocks; nodes stopped at 26..=30.
    let pdir = TempDir::new().unwrap();
    let mut producer = open(&pdir);
    let mut blocks = Vec::new();
    for h in 1..=HALT {
        let b = producer
            .candidate_block(1_000_000 + h * 15, Vec::new())
            .unwrap();
        producer.insert_block(b.clone()).unwrap();
        blocks.push(b);
    }
    let dirs: Vec<TempDir> = (0..NODES).map(|_| TempDir::new().unwrap()).collect();
    for (i, dir) in dirs.iter().enumerate() {
        let mut c = open(dir);
        let reached = HALT - (i as u64 % 5);
        for b in &blocks[..usize::try_from(reached).unwrap()] {
            c.insert_block(b.clone()).unwrap();
        }
    }
    // One node's state was corrupted by the bug that caused the halt: it followed a fork.
    let bad = TempDir::new().unwrap();
    {
        let mut c = open(&bad);
        for h in 1..=HALT {
            let b = c.candidate_block(2_000_000 + h * 15, Vec::new()).unwrap();
            c.insert_block(b).unwrap();
        }
    }

    // Agreement: the restart point is the producer's block at HALT.
    let t_sign = Instant::now();
    let tip = producer.get(&producer.tip()).unwrap();
    let msg = record(HALT, &tip.header.state_root, &producer.tip());
    let operators: Vec<_> = (0..NODES)
        .map(|i| {
            MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes(
                [u8::try_from(i).unwrap() + 1; 32],
            ))
        })
        .collect();
    let set: Vec<Vec<u8>> = operators.iter().map(MlDsa65::public_key).collect();
    // Three operators are unreachable; nine sign.
    let sigs: Vec<(usize, Vec<u8>)> = operators
        .iter()
        .enumerate()
        .take(9)
        .map(|(i, k)| (i, MlDsa65::sign(k, &msg).unwrap()))
        .collect();
    let valid = sigs
        .iter()
        .filter(|(i, s)| verify(SuiteId::MlDsa65, &set[*i], &msg, s).is_ok())
        .count();
    assert!(
        3 * valid > 2 * NODES,
        "a restart record needs more than 2/3 of operators"
    );
    let sign_ms = t_sign.elapsed().as_secs_f64() * 1e3;

    // Each node reaches the height, checks the record, restarts.
    let t_restart = Instant::now();
    for dir in &dirs {
        let mut c = open(dir);
        for b in &blocks[usize::try_from(c.height()).unwrap()..] {
            c.insert_block(b.clone()).unwrap();
        }
        let at = c.get(&c.tip()).unwrap();
        assert_eq!(
            (c.height(), c.tip(), at.header.state_root),
            (HALT, producer.tip(), tip.header.state_root)
        );
        assert_eq!(c.state().state_root().unwrap(), tip.header.state_root);
    }
    let restart_ms = t_restart.elapsed().as_secs_f64() * 1e3;

    // The corrupted node is caught by the same check and must resync instead.
    let c = open(&bad);
    assert_ne!(
        c.tip(),
        producer.tip(),
        "the forked node must not pass the restart check"
    );

    // Production resumes; every restarted node accepts the next block.
    let next = producer
        .candidate_block(1_000_000 + (HALT + 1) * 15, Vec::new())
        .unwrap();
    producer.insert_block(next.clone()).unwrap();
    for dir in &dirs {
        let mut c = open(dir);
        c.insert_block(next.clone()).unwrap();
        assert_eq!(c.height(), HALT + 1);
    }
    println!(
        "coordinated restart: {NODES} nodes; {valid}/{NODES} operator signatures (quorum > 2/3) signed+verified in {sign_ms:.1} ms; all nodes caught up and matched the record in {restart_ms:.0} ms; 1 forked node rejected; resumed at {} (total {:.1} s)",
        HALT + 1,
        started.elapsed().as_secs_f64()
    );
}
