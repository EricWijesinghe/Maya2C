//! Integration tests for the P2P layer.
//!
//! Three nodes are spawned on libp2p's in-process memory transport — no
//! sockets, no ports, no OS-level flakiness — while still exercising the real
//! noise + yamux + gossipsub + Kademlia stack.
//!
//! Topology is a line, `0 — 1 — 2`, so node 2 is reachable from node 0 only by
//! gossip relayed through node 1. That distinguishes actual mesh propagation
//! from direct delivery to a connected peer.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::generate_signing_key;
use custom_l1_node::network::{Node, NodeHandle};
use custom_l1_node::state::{Account, Address, StateDB};

use custom_l1_node::crypto::hybrid::HybridSigningKey;
use libp2p::Multiaddr;
use tempfile::TempDir;

/// Ceiling for any propagation wait. Reached only on failure.
const PROPAGATION_TIMEOUT: Duration = Duration::from_secs(20);

/// Poll interval while waiting for a condition.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// A spawned node plus the temp dir backing its state, which must outlive it.
struct TestNode {
    handle: NodeHandle,
    address: Multiaddr,
    _dir: TempDir,
}

/// Allocates memory-transport ports.
///
/// The memory transport's registry is process-wide and cargo runs the tests in
/// this binary concurrently, so fixed addresses collide across tests. A single
/// atomic counter gives every node in the process a unique port.
static NEXT_MEMORY_PORT: AtomicU64 = AtomicU64::new(1);

fn next_memory_address() -> Multiaddr {
    let port = NEXT_MEMORY_PORT.fetch_add(1, Ordering::Relaxed);
    format!("/memory/{port}").parse().expect("valid multiaddr")
}

/// Builds a node with `funded` accounts pre-credited, listening on a unique
/// in-memory address.
async fn spawn_node(funded: &[(Address, u64)]) -> TestNode {
    let dir = TempDir::new().expect("temp dir");
    let state = StateDB::open(dir.path()).expect("open state");

    for (address, balance) in funded {
        state
            .put_account(
                address,
                &Account {
                    balance: *balance,
                    nonce: 0,
                },
            )
            .expect("fund account");
    }

    let mut node = Node::new_memory(Arc::new(state)).expect("build node");

    let address = next_memory_address();
    node.listen_on(address.clone()).expect("listen");

    TestNode {
        handle: node.spawn(),
        address,
        _dir: dir,
    }
}

/// Polls a synchronous `condition` until true or the timeout elapses.
///
/// The condition must not block: it is evaluated on the test task, and the node
/// drivers need that task to yield at the `sleep` below in order to make
/// progress.
async fn wait_for<F>(label: &str, mut condition: F)
where
    F: FnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + PROPAGATION_TIMEOUT;
    loop {
        if condition() {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for: {label}");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Waits until `handle` reports at least `min` connected peers.
///
/// Separate from [`wait_for`] because querying peers is itself async — running
/// it through a blocking executor inside the runtime risks deadlocking against
/// the node driver task.
async fn wait_for_peers(handle: &NodeHandle, min: usize) {
    let deadline = tokio::time::Instant::now() + PROPAGATION_TIMEOUT;
    loop {
        let count = handle
            .connected_peers()
            .await
            .map(|peers| peers.len())
            .unwrap_or(0);
        if count >= min {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("timed out waiting for {min} peer(s); have {count}");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Publishes with retry.
///
/// Gossipsub rejects a publish with `InsufficientPeers` until the mesh for the
/// topic has formed, which is a normal startup race rather than a failure.
async fn publish_until_accepted(node: &NodeHandle, tx: &Transaction) {
    let deadline = tokio::time::Instant::now() + PROPAGATION_TIMEOUT;
    loop {
        match node.publish_transaction(tx).await {
            Ok(_) => return,
            Err(e) => {
                if tokio::time::Instant::now() >= deadline {
                    panic!("publish never succeeded: {e}");
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }
}

fn address_of(key: &HybridSigningKey) -> Address {
    key.address()
}

fn signed_transfer(from: &HybridSigningKey, to: Address, amount: u64, nonce: u64) -> Transaction {
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

/// Spawns three nodes wired as `0 — 1 — 2`, all sharing the same funded genesis.
async fn spawn_line_topology(funded: &[(Address, u64)]) -> Vec<TestNode> {
    let nodes = vec![
        spawn_node(funded).await,
        spawn_node(funded).await,
        spawn_node(funded).await,
    ];

    // Dial 0 -> 1 and 1 -> 2. Node 0 and node 2 never connect directly.
    nodes[0]
        .handle
        .dial(nodes[1].address.clone())
        .await
        .expect("dial 0->1");
    nodes[1]
        .handle
        .dial(nodes[2].address.clone())
        .await
        .expect("dial 1->2");

    // Seed Kademlia so discovery has a starting point.
    nodes[0]
        .handle
        .add_peer_address(nodes[1].handle.peer_id(), nodes[1].address.clone())
        .await
        .expect("seed kad 0");
    nodes[1]
        .handle
        .add_peer_address(nodes[2].handle.peer_id(), nodes[2].address.clone())
        .await
        .expect("seed kad 1");

    // Outer nodes have one peer each; the middle node bridges two.
    wait_for_peers(&nodes[0].handle, 1).await;
    wait_for_peers(&nodes[1].handle, 2).await;
    wait_for_peers(&nodes[2].handle, 1).await;

    nodes
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_nodes_connect_over_memory_transport() {
    let nodes = spawn_line_topology(&[]).await;

    let middle = nodes[1]
        .handle
        .connected_peers()
        .await
        .expect("connected peers");

    // The middle node bridges both ends.
    assert_eq!(middle.len(), 2, "middle node should link both outer nodes");
    assert!(middle.contains(&nodes[0].handle.peer_id()));
    assert!(middle.contains(&nodes[2].handle.peer_id()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gossiped_transaction_reaches_every_node() {
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let funded = [(alice_addr, 10_000u64)];

    let nodes = spawn_line_topology(&funded).await;

    let tx = signed_transfer(&alice, [2u8; 32], 500, 0);
    let txid = tx.txid();

    // Publishing also inserts locally, so all three pools should converge.
    nodes[0]
        .handle
        .mempool()
        .insert(tx.clone())
        .expect("local insert");
    publish_until_accepted(&nodes[0].handle, &tx).await;

    wait_for("node 1 to accept the transaction", || {
        nodes[1].handle.mempool().contains(&txid)
    })
    .await;

    // Node 2 is not connected to node 0; arrival proves relay through node 1.
    wait_for("node 2 to accept the relayed transaction", || {
        nodes[2].handle.mempool().contains(&txid)
    })
    .await;

    for (index, node) in nodes.iter().enumerate() {
        let pooled = node
            .handle
            .mempool()
            .get(&txid)
            .unwrap_or_else(|| panic!("node {index} should hold the transaction"));
        assert_eq!(pooled, tx, "node {index} stored a different transaction");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peers_reject_a_transaction_from_an_unfunded_sender() {
    let alice = generate_signing_key().expect("keygen");
    let pauper = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);

    // Only Alice is funded; the pauper has nothing anywhere.
    let nodes = spawn_line_topology(&[(alice_addr, 10_000u64)]).await;

    let funded_tx = signed_transfer(&alice, [2u8; 32], 100, 0);
    let funded_txid = funded_tx.txid();
    let broke_tx = signed_transfer(&pauper, [2u8; 32], 5_000, 0);
    let broke_txid = broke_tx.txid();

    // The unfunded transaction must not even enter the origin's own pool.
    assert!(
        nodes[0].handle.mempool().insert(broke_tx.clone()).is_err(),
        "origin should reject a transaction it cannot fund"
    );

    nodes[0]
        .handle
        .mempool()
        .insert(funded_tx.clone())
        .expect("funded insert");
    publish_until_accepted(&nodes[0].handle, &funded_tx).await;
    publish_until_accepted(&nodes[0].handle, &broke_tx).await;

    // Wait on the valid transaction; by the time it lands, the invalid one
    // gossiped alongside it has been seen and judged too.
    wait_for("node 2 to accept the funded transaction", || {
        nodes[2].handle.mempool().contains(&funded_txid)
    })
    .await;

    for (index, node) in nodes.iter().enumerate() {
        assert!(
            !node.handle.mempool().contains(&broke_txid),
            "node {index} admitted a transaction with no funds behind it"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peers_reject_a_tampered_transaction() {
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let nodes = spawn_line_topology(&[(alice_addr, 10_000u64)]).await;

    let good = signed_transfer(&alice, [2u8; 32], 100, 0);
    let good_txid = good.txid();

    // Mutate a signed transaction so its signature no longer matches.
    let mut tampered = signed_transfer(&alice, [2u8; 32], 100, 0);
    tampered.outputs[0].amount = 9_999;
    let tampered_txid = tampered.txid();

    nodes[0]
        .handle
        .mempool()
        .insert(good.clone())
        .expect("insert");
    publish_until_accepted(&nodes[0].handle, &good).await;
    publish_until_accepted(&nodes[0].handle, &tampered).await;

    wait_for("node 2 to accept the valid transaction", || {
        nodes[2].handle.mempool().contains(&good_txid)
    })
    .await;

    for (index, node) in nodes.iter().enumerate() {
        assert!(
            !node.handle.mempool().contains(&tampered_txid),
            "node {index} admitted a transaction with a broken signature"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kademlia_discovers_the_indirect_peer() {
    let nodes = spawn_line_topology(&[]).await;

    // Node 0 knows node 1 only. Bootstrapping plus identify should teach it
    // about node 2, which it has never dialed.
    nodes[0].handle.bootstrap().await.expect("bootstrap");

    let node_two = nodes[2].handle.peer_id();
    let mut events = nodes[0].handle.subscribe();

    let discovered = tokio::time::timeout(PROPAGATION_TIMEOUT, async {
        loop {
            match events.recv().await {
                Ok(custom_l1_node::network::NodeEvent::RoutingUpdated(peer)) => {
                    if peer == node_two {
                        return true;
                    }
                }
                Ok(_) => {}
                Err(_) => return false,
            }
        }
    })
    .await;

    assert!(
        matches!(discovered, Ok(true)),
        "node 0 should learn node 2 through the DHT without dialing it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]

mod common;
async fn duplicate_gossip_is_absorbed_without_growing_the_pool() {
    let alice = generate_signing_key().expect("keygen");
    let alice_addr = address_of(&alice);
    let nodes = spawn_line_topology(&[(alice_addr, 10_000u64)]).await;

    let tx = signed_transfer(&alice, [2u8; 32], 100, 0);
    let txid = tx.txid();

    nodes[0]
        .handle
        .mempool()
        .insert(tx.clone())
        .expect("insert");
    publish_until_accepted(&nodes[0].handle, &tx).await;

    wait_for("node 2 to receive the transaction", || {
        nodes[2].handle.mempool().contains(&txid)
    })
    .await;

    // Re-publishing the identical transaction must be idempotent.
    let _ = nodes[1].handle.publish_transaction(&tx).await;
    let _ = nodes[2].handle.publish_transaction(&tx).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    for (index, node) in nodes.iter().enumerate() {
        assert_eq!(
            node.handle.mempool().len(),
            1,
            "node {index} pool should hold exactly one transaction"
        );
    }
}
