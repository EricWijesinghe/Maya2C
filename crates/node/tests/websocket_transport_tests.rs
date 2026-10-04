//! Peers over WebSocket (approved 2026-10-04): a seed published through an
//! HTTP tunnel can only be reached as `/ws` (`/wss` at the tunnel's edge),
//! so outside validators dial it that way. The stream must still take the
//! full upgrade chain — Noise, the post-quantum handshake, yamux — exactly
//! as a TCP peer does; a WebSocket peer gets no weaker session.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use custom_l1_node::network::{Node, NodeEvent};
use custom_l1_node::state::db::StateDB;
use libp2p::Multiaddr;
use tempfile::TempDir;
use tokio::sync::broadcast::error::RecvError;

const TIMEOUT: Duration = Duration::from_secs(20);

fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port")
        .port()
}

async fn connected(events: &mut tokio::sync::broadcast::Receiver<NodeEvent>) -> bool {
    let wait = async {
        loop {
            match events.recv().await {
                Ok(NodeEvent::PeerConnected(_)) => return true,
                Ok(_) => {}
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return false,
            }
        }
    };
    tokio::time::timeout(TIMEOUT, wait).await.unwrap_or(false)
}

#[tokio::test]
async fn two_nodes_connect_over_websocket_with_the_post_quantum_handshake() {
    let (dir_a, dir_b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let mut seed = Node::new_tcp(Arc::new(StateDB::open(dir_a.path()).unwrap())).unwrap();
    let joiner = Node::new_tcp(Arc::new(StateDB::open(dir_b.path()).unwrap())).unwrap();

    // The seed listens on WebSocket only, as behind a tunnel: no TCP route.
    let ws: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}/ws", free_tcp_port())
        .parse()
        .unwrap();
    seed.listen_on(ws.clone()).unwrap();
    let (seed_stats, joiner_stats) = (seed.session_stats(), joiner.session_stats());

    let seed = seed.spawn();
    let joiner = joiner.spawn();
    let (mut seed_events, mut joiner_events) = (seed.subscribe(), joiner.subscribe());
    tokio::time::sleep(Duration::from_millis(300)).await; // listener up

    joiner.dial(ws).await.expect("dial over websocket");
    assert!(
        connected(&mut joiner_events).await,
        "joiner never connected"
    );
    assert!(
        connected(&mut seed_events).await,
        "seed never saw the joiner"
    );

    // Connected means the upgrade chain finished; the post-quantum session
    // is part of that chain, so both ends must have recorded one.
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    while (seed_stats.established() == 0 || joiner_stats.established() == 0)
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        seed_stats.established() >= 1,
        "seed: no post-quantum session"
    );
    assert!(
        joiner_stats.established() >= 1,
        "joiner: no post-quantum session"
    );
    assert_eq!(
        seed.connected_peers().await.expect("peers"),
        vec![joiner.peer_id()]
    );
}
