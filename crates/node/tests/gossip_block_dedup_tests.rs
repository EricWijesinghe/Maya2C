//! Gossiped blocks are relayed once per exact message, between two real nodes.
//!
//! Gossipsub's message id is author and sequence number, so a block
//! re-published under a fresh sequence number is a new message to it
//! (P2P-02). The node remembers what it accepted, and must key that on the
//! bytes: a block id alone does not commit to multisig approvals, which the
//! txid leaves out on purpose (`Transaction::txid`), so a copy with other
//! approvals carries the same header and must not shut the original out.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use custom_l1_node::core::block::BlockHeader;
use custom_l1_node::core::multisig_tx::MultisigAuth;
use custom_l1_node::core::{Block, Transaction, TxInput, TxOutput};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::{Node, NodeEvent, NodeHandle};
use custom_l1_node::state::StateDB;
use libp2p::Multiaddr;
use maya_crypto_pq::multisig::{Approval, MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{MasterSeed, MlDsa87, SignatureSuite};
use tempfile::TempDir;
use tokio::sync::broadcast;

mod common;

const TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

static NEXT_MEMORY_PORT: AtomicU64 = AtomicU64::new(9_610_000);

struct TestNode {
    handle: NodeHandle,
    address: Multiaddr,
    _dir: TempDir,
}

fn spawn_node() -> TestNode {
    let dir = TempDir::new().expect("temp dir");
    let state = StateDB::open(dir.path()).expect("open state");
    common::bind(&state);
    let mut node = Node::new_memory_with_rotation(Arc::new(state), Duration::from_secs(30))
        .expect("build node");
    let port = NEXT_MEMORY_PORT.fetch_add(1, Ordering::Relaxed);
    let address: Multiaddr = format!("/memory/{port}").parse().expect("multiaddr");
    node.listen_on(address.clone()).expect("listen");
    TestNode {
        handle: node.spawn(),
        address,
        _dir: dir,
    }
}

/// A connected pair whose mesh carries `a`'s publishes; returns `b`'s events.
async fn pair() -> (TestNode, TestNode, broadcast::Receiver<NodeEvent>) {
    let (a, b) = (spawn_node(), spawn_node());
    let events = b.handle.subscribe();
    b.handle.dial(a.address.clone()).await.expect("dial");
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    // The first publish succeeds once `b` is in `a`'s mesh; it is a marker
    // block nobody else uses.
    while a.handle.publish_block(&marker(0)).await.is_err() {
        assert!(tokio::time::Instant::now() < deadline, "mesh never formed");
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    (a, b, events)
}

fn header(nonce: u64) -> BlockHeader {
    BlockHeader {
        version: 0,
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp: 1_756_252_800,
        nonce,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    }
}

/// An empty block, told apart by its nonce.
fn marker(nonce: u64) -> Block {
    Block::new(header(nonce), vec![])
}

/// A block spending from a 1-of-1 multisig account, its one approval a real
/// signature, or that signature with one byte changed when `garbled`.
fn multisig_block(garbled: bool) -> Block {
    let key = MlDsa87::signing_key_from_seed(&MasterSeed::from_bytes([3; 32]));
    let listed = vec![PolicyKey {
        suite: MlDsa87::ID,
        public_key: MlDsa87::public_key(&key),
    }];
    let policy = MultisigPolicy::new(1, listed).expect("1-of-1");
    let mut tx = Transaction::new(
        vec![TxInput {
            prev_tx: [7; 32],
            index: 0,
        }],
        vec![TxOutput {
            amount: primitive_types::U256::from(40),
            recipient: [9; 32],
        }],
        3,
    );
    tx.multisig = Some(Box::new(MultisigAuth::unsigned(policy.clone())));
    let mut signature =
        MlDsa87::sign(&key, &tx.signing_bytes(&common::test_chain())).expect("sign");
    if garbled {
        signature[0] ^= 1;
    }
    let approvals = vec![Approval {
        index: 0,
        signature,
    }];
    tx.multisig = Some(Box::new(
        MultisigAuth::new(policy, approvals).expect("shape"),
    ));
    Block::new(header(77), vec![tx])
}

/// Every block body `b` accepted from gossip until `last` arrives, in order.
async fn received_until(events: &mut broadcast::Receiver<NodeEvent>, last: &Block) -> Vec<Vec<u8>> {
    let (last, mut seen) = (last.to_bytes(), Vec::new());
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(NodeEvent::TransactionRejected(why))) => {
                panic!("a test block was refused: {why}")
            }
            Ok(Ok(NodeEvent::BlockReceived { block, .. })) => {
                let bytes = block.to_bytes();
                if bytes == last {
                    return seen;
                }
                seen.push(bytes);
            }
            Ok(Ok(_) | Err(broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => {
                panic!(
                    "the closing marker never arrived; saw {} blocks",
                    seen.len()
                )
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_block_republished_under_a_new_sequence_number_is_relayed_once() {
    let (a, _b, mut events) = pair().await;
    let block = marker(5);
    // Two publishes are two gossip messages (fresh sequence numbers).
    a.handle.publish_block(&block).await.expect("publish");
    a.handle.publish_block(&block).await.expect("republish");
    let end = marker(6);
    a.handle.publish_block(&end).await.expect("publish end");

    let seen = received_until(&mut events, &end).await;
    let copies = seen.iter().filter(|b| **b == block.to_bytes()).count();
    assert_eq!(copies, 1, "an exact re-publication must be ignored");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_copy_with_other_approvals_does_not_shut_the_original_out() {
    let (a, _b, mut events) = pair().await;
    let honest = multisig_block(false);
    let tampered = multisig_block(true);
    // Draft #654 (audit CT-03) commits v8 approvals in `tx_root`, so the two
    // copies no longer share a header; de-duplication by message bytes must
    // keep the honest copy either way.
    assert_ne!(
        honest.header.id(),
        tampered.header.id(),
        "approvals are committed in tx_root (CT-03), so the header differs"
    );
    assert_ne!(honest.to_bytes(), tampered.to_bytes());

    // The tampered copy wins the race to `b`.
    a.handle
        .publish_block(&tampered)
        .await
        .expect("publish tampered");
    a.handle
        .publish_block(&honest)
        .await
        .expect("publish honest");
    let end = marker(8);
    a.handle.publish_block(&end).await.expect("publish end");

    let seen = received_until(&mut events, &end).await;
    assert!(
        seen.contains(&honest.to_bytes()),
        "the honest copy must still be accepted and relayed"
    );
}
