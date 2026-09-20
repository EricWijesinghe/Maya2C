//! Post-quantum transport, end to end through a running node.
//!
//! The unit tests in `custom_l1_node::network::pq` cover the handshake and the
//! framing in isolation. These cover the parts only a real swarm can show: that
//! the upgrade is actually applied to every connection, that the mandatory
//! negotiation has no fallback, and that session rotation both fires and
//! recovers.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use custom_l1_node::core::block::BlockHeader;
use custom_l1_node::core::{Block, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::pq::{PROTOCOL, PqUpgrade, ROTATION_INTERVAL_BLOCKS};
use custom_l1_node::network::{Node, NodeEvent, NodeHandle};
use custom_l1_node::state::StateDB;

use libp2p::Multiaddr;
use libp2p::core::UpgradeInfo;
use tempfile::TempDir;

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
    _dir: TempDir,
}

/// Spawns a node whose stale-session sweep runs every `rotation_check`.
async fn spawn_node(rotation_check: Duration) -> TestNode {
    let dir = TempDir::new().expect("temp dir");
    let state = StateDB::open(dir.path()).expect("open state");

    let mut node =
        Node::new_memory_with_rotation(Arc::new(state), rotation_check).expect("build node");
    let epoch = node.epoch_clock();

    let address = next_memory_address();
    node.listen_on(address.clone()).expect("listen");

    TestNode {
        handle: node.spawn(),
        address,
        epoch,
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
            amount: 77,
            recipient: [0x5Au8; 32],
        }],
        0,
    );
    tx.sign(&key).expect("sign");

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
fn the_upgrade_offers_no_alternative_protocol() {
    // A downgrade needs something to downgrade *to*. Offering exactly one
    // protocol is what makes multistream-select's failure mode "drop the
    // connection" rather than "negotiate something weaker".
    let offered: Vec<String> = PqUpgrade::new()
        .protocol_info()
        .map(|p| p.as_ref().to_string())
        .collect();

    assert_eq!(offered, vec![PROTOCOL.to_string()]);
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
