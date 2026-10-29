//! Post-quantum transport, end to end through a running node.
//!
//! The unit tests in `custom_l1_node::network::pq` cover the handshake and the
//! framing in isolation. These cover the parts only a real swarm can show: that
//! the upgrade is actually applied to every connection, that the mandatory
//! negotiation has no fallback, and that session rotation both fires and
//! recovers.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use custom_l1_node::core::block::BlockHeader;
use custom_l1_node::core::{Block, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::pq::{
    MlKem1024Policy, PROTOCOL, PROTOCOL_1024, PqProtocol, PqUpgrade, ROTATION_INTERVAL_BLOCKS,
};
use custom_l1_node::network::{Node, NodeEvent, NodeHandle};
use custom_l1_node::state::StateDB;

use libp2p::Multiaddr;
use libp2p::core::UpgradeInfo;
use tempfile::TempDir;

mod common;

const TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

static NEXT_MEMORY_PORT: AtomicU64 = AtomicU64::new(70_000);

fn next_memory_address() -> Multiaddr {
    let port = NEXT_MEMORY_PORT.fetch_add(1, Ordering::Relaxed);
    format!("/memory/{port}").parse().expect("valid multiaddr")
}

struct TestNode {
    handle: NodeHandle,
    address: Multiaddr,
    epoch: custom_l1_node::network::EpochClock,
    stats: custom_l1_node::network::SessionStats,
    _dir: TempDir,
}

/// Spawns a node whose stale-session sweep runs every `rotation_check`.
async fn spawn_node(rotation_check: Duration) -> TestNode {
    let dir = TempDir::new().expect("temp dir");
    let state = StateDB::open(dir.path()).expect("open state");
    common::bind(&state);

    let mut node =
        Node::new_memory_with_rotation(Arc::new(state), rotation_check).expect("build node");
    let epoch = node.epoch_clock();
    let stats = node.session_stats();

    let address = next_memory_address();
    node.listen_on(address.clone()).expect("listen");

    TestNode {
        handle: node.spawn(),
        address,
        epoch,
        stats,
        _dir: dir,
    }
}

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

fn signed_block() -> Block {
    let key = signing_key_from_seed(&[9u8; 32]).expect("derive");
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: primitive_types::U256::from(77),
            recipient: [0x5Au8; 32],
        }],
        0,
    );
    tx.sign(&key, &common::test_chain()).expect("sign");

    Block::new(
        BlockHeader {
            version: 0,
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

// ---------------------------------------------------------------------------
// the upgrade is applied, and is mandatory
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn two_nodes_connect_over_the_post_quantum_upgrade() {
    // The upgrade sits between authentication and multiplexing, so a connection
    // that reaches "established" has completed the ML-KEM exchange by
    // construction. There is no configuration under which it is skipped.
    let a = spawn_node(Duration::from_secs(30)).await;
    let b = spawn_node(Duration::from_secs(30)).await;

    b.handle.dial(a.address.clone()).await.expect("dial");

    let handle = b.handle.clone();
    assert!(
        wait_for(TIMEOUT, async || {
            handle
                .connected_peers()
                .await
                .map(|peers| !peers.is_empty())
                .unwrap_or(false)
        })
        .await,
        "the ML-KEM upgrade did not complete"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_block_survives_the_encrypted_transport_intact() {
    let a = spawn_node(Duration::from_secs(30)).await;
    let b = spawn_node(Duration::from_secs(30)).await;
    b.handle.dial(a.address.clone()).await.expect("dial");

    let mut receiver = b.handle.subscribe();
    let block = signed_block();
    let expected = block.to_bytes();

    let publisher = a.handle.clone();
    let payload = block.clone();
    assert!(
        wait_for(TIMEOUT, async || {
            publisher.publish_block(&payload).await.is_ok()
        })
        .await,
        "the mesh never accepted a publish"
    );

    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        match tokio::time::timeout_at(deadline, receiver.recv()).await {
            Ok(Ok(NodeEvent::BlockReceived {
                block: received, ..
            })) => {
                // Sealed, shipped, opened, and byte-identical. A framing or
                // nonce bug would corrupt this rather than merely slow it.
                assert_eq!(received.to_bytes(), expected);
                return;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) | Err(_) => panic!("block never arrived over the encrypted transport"),
        }
    }
}

#[test]
fn the_upgrade_offers_only_the_two_ml_kem_protocols() {
    // Level 5 first, level 3 as the fallback that keeps older peers connecting.
    // Nothing else is ever offered, so there is no third exchange to downgrade to.
    let offered: Vec<String> = PqUpgrade::new()
        .protocol_info()
        .map(|p| p.as_ref().to_string())
        .collect();

    assert_eq!(
        offered,
        vec![PROTOCOL_1024.to_string(), PROTOCOL.to_string()]
    );
}

// ---------------------------------------------------------------------------
// ML-KEM-1024 negotiation (ADR-051 option 1)
// ---------------------------------------------------------------------------

/// How long a pair is watched before a non-connection is called a refusal.
const REFUSAL_WINDOW: Duration = Duration::from_secs(3);

/// Spawns a memory-transport node offering the given post-quantum upgrade.
fn spawn_with_pq(pq: PqUpgrade) -> TestNode {
    let dir = TempDir::new().expect("temp dir");
    let state = StateDB::open(dir.path()).expect("open state");
    common::bind(&state);

    let mut node = Node::new_memory_with_pq(Arc::new(state), pq).expect("build node");
    let epoch = node.epoch_clock();
    let stats = node.session_stats();

    let address = next_memory_address();
    node.listen_on(address.clone()).expect("listen");

    TestNode {
        handle: node.spawn(),
        address,
        epoch,
        stats,
        _dir: dir,
    }
}

/// Spawns a listener and a dialer, and dials. Returns both for inspection.
async fn dial_pair(dialer: PqUpgrade, listener: PqUpgrade) -> (TestNode, TestNode) {
    let server = spawn_with_pq(listener);
    let client = spawn_with_pq(dialer);
    // A dial that races the listener's bind fails here and is not retried, so
    // callers assert on the connection count, not on this result.
    let _ = client.handle.dial(server.address.clone()).await;
    (client, server)
}

async fn peer_count(handle: &NodeHandle) -> usize {
    handle.connected_peers().await.map(|p| p.len()).unwrap_or(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn two_mlkem1024_nodes_negotiate_1024() {
    let (client, server) = dial_pair(PqUpgrade::new(), PqUpgrade::new()).await;

    assert!(
        wait_for(TIMEOUT, async || peer_count(&client.handle).await > 0).await,
        "two level-5 nodes did not connect"
    );
    // Both ends of the one connection must record the same exchange, and it
    // must be the level-5 one.
    for side in [&client, &server] {
        let stats = side.stats.clone();
        assert!(
            wait_for(TIMEOUT, async || stats.negotiated(PqProtocol::MlKem1024)
                >= 1)
            .await,
            "a side did not record an ML-KEM-1024 session"
        );
        assert_eq!(stats.negotiated(PqProtocol::MlKem768), 0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_mlkem1024_node_falls_back_to_a_768_only_peer() {
    // The listener is a level-3-only node, standing in for a build from before
    // ML-KEM-1024 existed. The dialer prefers 1024, must fall back, and connect.
    let legacy = PqUpgrade::new().with_mlkem1024(MlKem1024Policy::Off);
    let (client, server) = dial_pair(PqUpgrade::new(), legacy).await;

    assert!(
        wait_for(TIMEOUT, async || peer_count(&client.handle).await > 0).await,
        "a level-5 node did not fall back to a level-3 peer"
    );
    for side in [&client, &server] {
        let stats = side.stats.clone();
        assert!(
            wait_for(TIMEOUT, async || stats.negotiated(PqProtocol::MlKem768)
                >= 1)
            .await,
            "a side did not record the ML-KEM-768 fallback"
        );
        assert_eq!(stats.negotiated(PqProtocol::MlKem1024), 0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_level3_dialer_still_reaches_a_level5_node() {
    // The other direction of the same mixed network: the old node dials. Both
    // ends must record the level-3 exchange, which is what the connection used.
    let legacy = PqUpgrade::new().with_mlkem1024(MlKem1024Policy::Off);
    let (client, server) = dial_pair(legacy, PqUpgrade::new()).await;

    assert!(
        wait_for(TIMEOUT, async || peer_count(&client.handle).await > 0).await,
        "a level-3 node could not reach a level-5 node"
    );
    for side in [&client, &server] {
        let stats = side.stats.clone();
        assert!(
            wait_for(TIMEOUT, async || stats.negotiated(PqProtocol::MlKem768)
                >= 1)
            .await,
            "a side did not record the ML-KEM-768 exchange"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_1024_required_node_refuses_a_768_only_peer() {
    // Required offers no fallback, so the two sets share nothing. The
    // connection must not form. A refusal proves nothing unless the same
    // listener accepts a dialer that can speak, so a level-5 control dials
    // first and must connect. Then the refusal can only be the policy.
    let server = spawn_with_pq(PqUpgrade::new().with_mlkem1024(MlKem1024Policy::Off));
    let control = spawn_with_pq(PqUpgrade::new());
    let _ = control.handle.dial(server.address.clone()).await;
    assert!(
        wait_for(TIMEOUT, async || peer_count(&control.handle).await > 0).await,
        "positive control failed: a level-5 dialer could not reach the listener"
    );

    let required = spawn_with_pq(PqUpgrade::new().with_mlkem1024(MlKem1024Policy::Required));
    let _ = required.handle.dial(server.address.clone()).await;
    tokio::time::sleep(REFUSAL_WINDOW).await;

    assert_eq!(
        peer_count(&required.handle).await,
        0,
        "level 5 fell back to level 3"
    );
    // Only the control's session was negotiated; the refused dial left no trace.
    assert_eq!(server.stats.negotiated(PqProtocol::MlKem768), 1);
    assert_eq!(server.stats.negotiated(PqProtocol::MlKem1024), 0);
}

// ---------------------------------------------------------------------------
// rotation
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn advancing_an_epoch_rotates_the_session() {
    // The whole rotation mechanism, observed rather than reasoned about: a
    // connection established in one epoch is closed once the node's height
    // crosses into the next.
    let sweep = Duration::from_millis(200);
    let a = spawn_node(sweep).await;
    let b = spawn_node(sweep).await;

    let mut disconnects = b.handle.subscribe();
    b.handle.dial(a.address.clone()).await.expect("dial");

    let handle = b.handle.clone();
    assert!(
        wait_for(TIMEOUT, async || {
            handle
                .connected_peers()
                .await
                .map(|peers| !peers.is_empty())
                .unwrap_or(false)
        })
        .await,
        "nodes never connected"
    );

    // Cross an epoch boundary on both sides. Either would do — whichever
    // notices first closes the connection — but advancing both is what a real
    // network does, and it exercises the case where they agree.
    a.epoch.set_height(ROTATION_INTERVAL_BLOCKS);
    b.epoch.set_height(ROTATION_INTERVAL_BLOCKS);
    assert_eq!(b.epoch.current(), 1);

    let deadline = tokio::time::Instant::now() + TIMEOUT;
    let mut rotated = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout_at(deadline, disconnects.recv()).await {
            Ok(Ok(NodeEvent::PeerDisconnected(_))) => {
                rotated = true;
                break;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) | Err(_) => break,
        }
    }

    assert!(
        rotated,
        "crossing an epoch boundary did not close the stale session"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_within_its_epoch_is_left_alone() {
    // The other half of the property, and the one a bug would break silently:
    // rotation must not churn connections that are still fresh. A sweep that
    // closed everything every tick would still pass the test above.
    let sweep = Duration::from_millis(100);
    let a = spawn_node(sweep).await;
    let b = spawn_node(sweep).await;

    b.handle.dial(a.address.clone()).await.expect("dial");

    let handle = b.handle.clone();
    assert!(
        wait_for(TIMEOUT, async || {
            handle
                .connected_peers()
                .await
                .map(|peers| !peers.is_empty())
                .unwrap_or(false)
        })
        .await,
        "nodes never connected"
    );

    // Well inside epoch 0, across many sweeps.
    b.epoch.set_height(ROTATION_INTERVAL_BLOCKS - 1);
    tokio::time::sleep(sweep * 10).await;

    let peers = b.handle.connected_peers().await.expect("peers");
    assert!(
        !peers.is_empty(),
        "a session still inside its epoch was rotated anyway"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rotated_session_comes_back() {
    // Rotation is only useful if the mesh heals. Closing the connection is the
    // easy half; the point is that libp2p redials and the ML-KEM upgrade runs
    // again, leaving the node connected with fresh keys.
    let sweep = Duration::from_millis(200);
    let a = spawn_node(sweep).await;
    let b = spawn_node(sweep).await;

    b.handle.dial(a.address.clone()).await.expect("dial");

    let handle = b.handle.clone();
    let connected = async || {
        handle
            .connected_peers()
            .await
            .map(|peers| !peers.is_empty())
            .unwrap_or(false)
    };

    assert!(wait_for(TIMEOUT, connected).await, "nodes never connected");

    a.epoch.set_height(ROTATION_INTERVAL_BLOCKS * 2);
    b.epoch.set_height(ROTATION_INTERVAL_BLOCKS * 2);

    // Redial explicitly. In a deployment Kademlia and gossipsub bring the peer
    // back on their own schedule; the test drives it rather than waiting on a
    // bootstrap interval it does not control.
    tokio::time::sleep(sweep * 3).await;
    let _ = b.handle.dial(a.address.clone()).await;

    let handle = b.handle.clone();
    assert!(
        wait_for(TIMEOUT, async || {
            handle
                .connected_peers()
                .await
                .map(|peers| !peers.is_empty())
                .unwrap_or(false)
        })
        .await,
        "the mesh did not heal after rotation"
    );
}
