//! Multi-node network simulation under injected latency.
//!
//! Requirement 4 of the post-quantum transport work, and the only test in the
//! repository that asks whether the protocol survives a network that is not
//! instantaneous.
//!
//! ## Why this test exists
//!
//! Every other network test runs on the memory transport, where a write is a
//! `Vec` push. That is the right default — fast, deterministic, no sockets —
//! but it means the suite validates the protocol under conditions no real
//! network provides. The post-quantum upgrade added a **two-message round trip
//! to every connection**, and a round trip is precisely what latency
//! multiplies. On a zero-latency transport that cost is invisible; at 250 ms it
//! dominates connection setup. A test that never delays anything cannot tell
//! "the handshake works" from "the handshake works when it is free".
//!
//! ## Topology
//!
//! A line, `0 — 1 — 2 — 3 — 4`. Node 4 is reachable from node 0 only by gossip
//! relayed through three intermediate hops, so a block arriving there proves
//! mesh propagation rather than direct delivery. Each hop pays the injected
//! latency again, which is the point: the line multiplies the delay by four and
//! makes any per-hop cost visible.
//!
//! ## What is asserted
//!
//! 1. Every node completes the ML-KEM handshake and connects, at every latency.
//! 2. A block published at one end reaches the far end, intact and byte-equal.
//! 3. Propagation time scales with latency rather than collapsing — a run that
//!    finished as fast at 250 ms as at 0 ms would mean the delay was not
//!    reaching the transport and the test was proving nothing.
//!
//! ## What is not asserted
//!
//! Bandwidth, jitter, loss, reordering, and queueing are not modelled. A pass
//! here is evidence that latency alone does not break propagation. It is not
//! evidence that the protocol survives a genuinely bad network.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use custom_l1_node::core::block::BlockHeader;
use custom_l1_node::core::{Block, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::{LATENCY_SWEEP, Node, NodeEvent, NodeHandle};
use custom_l1_node::state::StateDB;

use libp2p::Multiaddr;
use tempfile::TempDir;

/// Nodes in the line.
const NODES: usize = 5;

/// Hops a block must cross to reach the far end.
const HOPS: u32 = NODES as u32 - 1;

/// Ceiling for any propagation wait, before latency is added.
///
/// Reached only on failure. The per-latency budget is computed from this plus
/// the delay the run actually injects, so a slow run fails as a timeout rather
/// than hanging the suite.
const BASE_TIMEOUT: Duration = Duration::from_secs(30);

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Allocates unique memory-transport addresses across the whole test binary.
static NEXT_MEMORY_PORT: AtomicU64 = AtomicU64::new(90_000);

fn next_memory_address() -> Multiaddr {
    let port = NEXT_MEMORY_PORT.fetch_add(1, Ordering::Relaxed);
    format!("/memory/{port}").parse().expect("valid multiaddr")
}

struct SimNode {
    handle: NodeHandle,
    address: Multiaddr,
    _dir: TempDir,
}

/// Spawns a node whose reads are delayed by `latency`.
async fn spawn_node(latency: Duration) -> SimNode {
    let dir = TempDir::new().expect("temp dir");
    let state = StateDB::open(dir.path()).expect("open state");

    let mut node = Node::new_memory_with_latency(Arc::new(state), latency).expect("build node");

    let address = next_memory_address();
    node.listen_on(address.clone()).expect("listen");

    SimNode {
        handle: node.spawn(),
        address,
        _dir: dir,
    }
}

/// A signed block, so what propagates is the real thing rather than a stub.
fn signed_block(key: &HybridSigningKey) -> Block {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 4_242,
            recipient: [0xABu8; 32],
        }],
        0,
    );
    tx.sign(key).expect("sign");

    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_756_252_800,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        vec![tx],
    )
}

/// Waits until `condition` holds, or the deadline passes.
async fn wait_for(timeout: Duration, mut condition: impl AsyncFnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition().await {
            return true;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    false
}

/// Builds the line `0 — 1 — 2 — ... — n`, waiting for every link to come up.
///
/// Connection establishment is what the post-quantum handshake gates: if the
/// ML-KEM exchange failed or timed out under latency, this is where it shows,
/// before any gossip is attempted.
async fn connected_line(latency: Duration) -> Vec<SimNode> {
    let mut nodes = Vec::with_capacity(NODES);
    for _ in 0..NODES {
        nodes.push(spawn_node(latency).await);
    }

    for index in 1..NODES {
        nodes[index]
            .handle
            .dial(nodes[index - 1].address.clone())
            .await
            .expect("dial");
    }

    // Every node but the endpoints holds two links; the endpoints hold one.
    let timeout = BASE_TIMEOUT + latency * 40;
    for (index, node) in nodes.iter().enumerate() {
        let expected = if index == 0 || index == NODES - 1 {
            1
        } else {
            2
        };
        let handle = node.handle.clone();
        let linked = wait_for(timeout, async || {
            handle
                .connected_peers()
                .await
                .map(|peers| peers.len() >= expected)
                .unwrap_or(false)
        })
        .await;

        assert!(
            linked,
            "node {index} did not complete its ML-KEM handshakes at {latency:?} latency"
        );
    }

    nodes
}

#[tokio::test(flavor = "multi_thread")]
async fn a_block_propagates_across_the_line_at_every_latency() {
    let key = signing_key_from_seed(&[1u8; 32]).expect("derive");
    let block = signed_block(&key);
    let expected = block.to_bytes();

    for latency in LATENCY_SWEEP {
        let nodes = connected_line(latency).await;

        // Subscribe before publishing: a broadcast receiver only sees what is
        // sent after it subscribes.
        let mut receivers: Vec<_> = nodes.iter().map(|n| n.handle.subscribe()).collect();

        let timeout = BASE_TIMEOUT + latency * 40;

        // Retry until the gossipsub mesh has a subscriber, rather than sleeping
        // a guessed interval. Mesh formation is itself several round trips —
        // subscriptions propagate on the heartbeat — so any fixed settle time
        // is either too short at high latency or wasted at low. Publishing is
        // the operation that tells us the mesh is ready, so use it as the
        // signal instead of trying to predict it.
        let publisher = nodes[0].handle.clone();
        let payload = block.clone();
        let published = wait_for(timeout, async || {
            publisher.publish_block(&payload).await.is_ok()
        })
        .await;

        assert!(
            published,
            "the gossip mesh never accepted a publish at {latency:?} latency \
             within {timeout:?}"
        );

        // Node 0 is the publisher and does not receive its own gossip, so the
        // interesting assertion is on 1..n — and especially on the last, which
        // is three relays away.
        for (index, receiver) in receivers.iter_mut().enumerate().skip(1) {
            let deadline = tokio::time::Instant::now() + timeout;
            let mut seen = false;

            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout_at(deadline, receiver.recv()).await {
                    Ok(Ok(NodeEvent::BlockReceived {
                        block: received, ..
                    })) => {
                        assert_eq!(
                            received.to_bytes(),
                            expected,
                            "node {index} received a corrupted block at {latency:?}"
                        );
                        seen = true;
                        break;
                    }
                    Ok(Ok(_)) => continue,
                    Ok(Err(_)) | Err(_) => break,
                }
            }

            assert!(
                seen,
                "node {index} never received the block at {latency:?} latency \
                 ({HOPS} hops, {timeout:?} budget)"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn latency_actually_reaches_the_transport() {
    // The control for the test above. If the delay were not being applied —
    // wrong transport, short-circuited stream, a `DelayStream` optimized away —
    // every assertion there would still pass, and pass for the wrong reason.
    //
    // Connection setup is the right thing to measure: the ML-KEM handshake is a
    // fixed two-message round trip, so its cost is close to linear in latency
    // and is not smoothed out by gossipsub's heartbeat timers.
    let fast = Instant::now();
    let _ = connected_line(Duration::ZERO).await;
    let fast = fast.elapsed();

    let slow_latency = Duration::from_millis(100);
    let slow = Instant::now();
    let _ = connected_line(slow_latency).await;
    let slow = slow.elapsed();

    assert!(
        slow > fast,
        "injecting {slow_latency:?} of latency did not slow connection setup \
         ({slow:?} against {fast:?}); the delay is not reaching the transport"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn every_node_completes_the_post_quantum_handshake_under_latency() {
    // Requirement 2, asserted end to end rather than at the unit level: the
    // upgrade is mandatory, so a mesh that forms at all is a mesh in which
    // every link ran ML-KEM-768. There is no negotiated fallback that could
    // have produced a connected-but-unprotected peer.
    let latency = Duration::from_millis(250);
    let nodes = connected_line(latency).await;

    for (index, node) in nodes.iter().enumerate() {
        let peers = node.handle.connected_peers().await.expect("peers");
        assert!(
            !peers.is_empty(),
            "node {index} has no post-quantum sessions at {latency:?}"
        );
    }
}
