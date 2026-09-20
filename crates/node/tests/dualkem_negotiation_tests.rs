//! Node-to-node negotiation of `/maya/dualkem/1.0.0`.
//!
//! The protocol is negotiated per connection rather than required, so what
//! matters is not "does the dual handshake work" — `dualkem_latency_tests.rs`
//! covers that — but **which pairs of policies can talk to each other at all**.
//!
//! Every combination is exercised here, because the interesting cases are the
//! asymmetric ones. A `Required` node and a `Disabled` node share no protocol
//! and must fail to connect; if that ever silently succeeded it would mean a
//! node told to require post-quantum belt-and-braces had quietly fallen back
//! to one of them.
//!
//! # Why connection success is the assertion, and not the negotiated name
//!
//! libp2p does not surface the transport-upgrade protocol that was selected —
//! `ConnectionEstablished` carries the peer and the endpoint, not the inner
//! upgrade's name. So these tests assert the observable consequence: whether a
//! connection forms. `network::pq::mod`'s unit tests assert the protocol *lists*
//! directly, and together the two pin the behaviour from both ends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use custom_l1_node::network::pq::dual::DualKemPolicy;
use custom_l1_node::network::{Node, NodeHandle};
use custom_l1_node::state::db::StateDB;
use libp2p::Multiaddr;
use tempfile::TempDir;

/// How long a connection is given to form before it is called a failure.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a connection is watched for before it is called a non-connection.
///
/// Shorter than [`CONNECT_TIMEOUT`] on purpose: this bounds a *negative*
/// assertion, and a negative that waits ten seconds makes the suite slow to
/// prove something that fails immediately when it fails at all. Long enough
/// that a merely slow handshake is not mistaken for a refused one.
const REFUSAL_WINDOW: Duration = Duration::from_secs(3);

const POLL_INTERVAL: Duration = Duration::from_millis(50);

struct TestNode {
    handle: NodeHandle,
    address: Multiaddr,
    _dir: TempDir,
}

/// Allocates memory-transport ports.
///
/// The registry is process-wide and cargo runs a binary's tests concurrently,
/// so fixed addresses collide. One atomic counter gives every node in the
/// process a unique port. Deliberately not shared with `network_tests.rs`:
/// that is a separate binary with its own address space, so the two counters
/// cannot conflict.
/// Based high rather than at 1: a memory address is a bare integer, so the
/// only way to keep two test binaries from colliding — if they ever share a
/// process — is to start them in different ranges.
static NEXT_MEMORY_PORT: AtomicU64 = AtomicU64::new(900_000);

fn next_memory_address() -> Multiaddr {
    let port = NEXT_MEMORY_PORT.fetch_add(1, Ordering::Relaxed);
    format!("/memory/{port}").parse().expect("valid multiaddr")
}

/// Spawns a node on the memory transport under an explicit dual-KEM policy.
async fn spawn_node(policy: DualKemPolicy) -> TestNode {
    let dir = TempDir::new().expect("temp dir");
    let state = StateDB::open(dir.path()).expect("open state");

    let mut node = Node::new_memory_with_dual_kem(Arc::new(state), policy).expect("build node");

    let address = next_memory_address();
    node.listen_on(address.clone()).expect("listen");

    TestNode {
        handle: node.spawn(),
        address,
        _dir: dir,
    }
}

/// Whether `handle` reaches at least one peer within `window`.
async fn connects_within(handle: &NodeHandle, window: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let peers = handle
            .connected_peers()
            .await
            .map(|peers| peers.len())
            .unwrap_or(0);
        if peers > 0 {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Dials `listener` from `dialer` and reports whether a connection formed.
async fn try_connect(dialer: DualKemPolicy, listener: DualKemPolicy, window: Duration) -> bool {
    let server = spawn_node(listener).await;
    let client = spawn_node(dialer).await;

    // A dial to an address nothing is listening on yet is a normal startup
    // race, so the failure to assert on is the absence of a peer afterwards,
    // not an error from `dial` itself.
    let _ = client.handle.dial(server.address.clone()).await;

    connects_within(&client.handle, window).await
}

// ---------------------------------------------------------------------------
// Symmetric pairs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_disabled_nodes_connect() {
    // The status quo. Every node in the network today is this one, and adding
    // the dual protocol must not have changed it.
    assert!(
        try_connect(
            DualKemPolicy::Disabled,
            DualKemPolicy::Disabled,
            CONNECT_TIMEOUT
        )
        .await,
        "two stock nodes must still connect"
    );
}

#[tokio::test]
async fn two_preferred_nodes_connect() {
    assert!(
        try_connect(
            DualKemPolicy::Offered,
            DualKemPolicy::Offered,
            CONNECT_TIMEOUT
        )
        .await,
        "two opted-in nodes must connect"
    );
}

#[tokio::test]
async fn two_required_nodes_connect() {
    // The end state the rollout is aiming at.
    assert!(
        try_connect(
            DualKemPolicy::Required,
            DualKemPolicy::Required,
            CONNECT_TIMEOUT
        )
        .await,
        "two required nodes must connect"
    );
}

// ---------------------------------------------------------------------------
// Mixed pairs — the rollout cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_preferred_node_connects_to_a_disabled_one() {
    // The property that makes incremental rollout possible: an operator can
    // turn this on for one node without partitioning it from the network.
    assert!(
        try_connect(
            DualKemPolicy::Offered,
            DualKemPolicy::Disabled,
            CONNECT_TIMEOUT
        )
        .await,
        "an opted-in node must still reach a stock node"
    );
}

#[tokio::test]
async fn a_disabled_node_connects_to_a_preferred_one() {
    // The same, dialled the other way. Negotiation is not symmetric in libp2p
    // — the dialer proposes and the listener selects — so both directions are
    // separate cases rather than one.
    assert!(
        try_connect(
            DualKemPolicy::Disabled,
            DualKemPolicy::Offered,
            CONNECT_TIMEOUT
        )
        .await,
        "a stock node must still reach an opted-in node"
    );
}

#[tokio::test]
async fn a_required_node_connects_to_a_preferred_one() {
    assert!(
        try_connect(
            DualKemPolicy::Required,
            DualKemPolicy::Offered,
            CONNECT_TIMEOUT
        )
        .await,
        "required and preferred share the dual protocol"
    );
}

// ---------------------------------------------------------------------------
// The refusal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_required_node_refuses_a_disabled_one() {
    // The assertion the `Required` policy exists for. The two offer disjoint
    // protocol sets, so negotiation fails and the connection drops.
    //
    // If this ever passed, a node configured to require both KEMs would have
    // silently accepted one — which is the failure mode an operator setting
    // this flag is specifically trying to rule out.
    assert!(
        !try_connect(
            DualKemPolicy::Required,
            DualKemPolicy::Disabled,
            REFUSAL_WINDOW
        )
        .await,
        "a required node must not fall back to the single-KEM protocol"
    );
}

#[tokio::test]
async fn a_disabled_node_is_refused_by_a_required_one() {
    // The same disjointness, dialled the other way.
    assert!(
        !try_connect(
            DualKemPolicy::Disabled,
            DualKemPolicy::Required,
            REFUSAL_WINDOW
        )
        .await,
        "a stock node must not be admitted by a required node"
    );
}
