//! Network separation at the handshake: a node bound to one genesis must
//! refuse a peer bound to another, before any gossip, sync or DHT traffic.
//!
//! The protocol names alone do not separate chains (`/l1/blocks/1.0.0` is the
//! same on testnet and mainnet), so the separation is carried by the Noise
//! prologue, which both ends must hash identically or the handshake fails.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use custom_l1_node::core::ChainTag;
use custom_l1_node::network::{Node, NodeEvent, NodeHandle};
use custom_l1_node::state::db::StateDB;
use libp2p::Multiaddr;
use tempfile::TempDir;
use tokio::sync::broadcast::error::RecvError;

const CONNECT_WAIT: Duration = Duration::from_secs(20);
const REFUSAL_WAIT: Duration = Duration::from_secs(5);

fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port")
        .port()
}

/// Starts a listening seed bound to `seed_tag` and a dialer bound to `joiner_tag`.
async fn spawn_pair(
    seed_tag: [u8; 32],
    joiner_tag: [u8; 32],
) -> (NodeHandle, NodeHandle, Multiaddr) {
    let (dir_a, dir_b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let seed_state = Arc::new(StateDB::open(dir_a.path()).unwrap());
    let joiner_state = Arc::new(StateDB::open(dir_b.path()).unwrap());
    seed_state.bind_chain(ChainTag(seed_tag)).unwrap();
    joiner_state.bind_chain(ChainTag(joiner_tag)).unwrap();

    let mut seed = Node::new_tcp(seed_state).unwrap();
    let joiner = Node::new_tcp(joiner_state).unwrap();
    let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", free_tcp_port())
        .parse()
        .unwrap();
    seed.listen_on(addr.clone()).unwrap();

    let seed = seed.spawn();
    let joiner = joiner.spawn();
    // The listener must be up before the dial, or the first attempt is refused for timing.
    tokio::time::sleep(Duration::from_millis(300)).await;
    (seed, joiner, addr)
}

/// True if a `PeerConnected` arrives within `wait`.
async fn connects_within(
    events: &mut tokio::sync::broadcast::Receiver<NodeEvent>,
    wait: Duration,
) -> bool {
    let next = async {
        loop {
            match events.recv().await {
                Ok(NodeEvent::PeerConnected(_)) => return true,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return false,
            }
        }
    };
    tokio::time::timeout(wait, next).await.unwrap_or(false)
}

#[tokio::test]
async fn nodes_on_the_same_genesis_connect() {
    let (seed, joiner, addr) = spawn_pair([7; 32], [7; 32]).await;
    let mut joiner_events = joiner.subscribe();

    joiner.dial(addr).await.expect("dial");
    assert!(
        connects_within(&mut joiner_events, CONNECT_WAIT).await,
        "same genesis must connect"
    );
    assert_eq!(
        seed.connected_peers().await.expect("peers"),
        vec![joiner.peer_id()]
    );
}

#[tokio::test]
async fn nodes_on_different_geneses_refuse_each_other_at_the_handshake() {
    // Testnet and mainnet differ only in their genesis id.
    let (seed, joiner, addr) = spawn_pair([1; 32], [2; 32]).await;
    let mut joiner_events = joiner.subscribe();

    joiner
        .dial(addr)
        .await
        .expect("dial is queued, the refusal comes later");
    assert!(
        !connects_within(&mut joiner_events, REFUSAL_WAIT).await,
        "a peer on another genesis must not complete the handshake"
    );
    assert!(
        seed.connected_peers().await.expect("peers").is_empty(),
        "the listener must hold no session with a foreign genesis"
    );
}
