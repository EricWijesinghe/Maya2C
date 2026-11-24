//! The per-source handshake cap (`network::ip_limits`) on a real TCP node.
//!
//! The unit tests call the behaviour's handlers directly; this checks it is
//! composed into the swarm, that a source over its cap is refused at accept,
//! and that the slots stalled handshakes held come back once the connection
//! timeout ends them. Loopback is exempt from the cap, so the handshakes come
//! from this machine's own non-loopback address; on a host without one the
//! test says so and passes vacuously.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::Duration;

use custom_l1_node::network::Node;
use custom_l1_node::network::ip_limits::MAX_PENDING_PER_SOURCE;
use custom_l1_node::state::StateDB;
use libp2p::Multiaddr;
use tempfile::TempDir;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

/// Longer than the node's whole-upgrade deadline (10 s, `node::build`).
const PAST_CONNECTION_TIMEOUT: Duration = Duration::from_secs(14);

/// How long a refused socket may take to see the node close it.
const REFUSAL_PATIENCE: Duration = Duration::from_secs(3);

/// This host's address on its default route, if it has one. No packet is
/// sent: connecting a UDP socket only picks the route.
fn non_loopback_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

fn free_tcp_port() -> u16 {
    TcpListener::bind("0.0.0.0:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port")
        .port()
}

/// Whether the node closed `socket` within `patience`. A socket whose
/// handshake is merely pending stays open (it may carry the node's
/// multistream header, never an end of stream).
async fn closed_within(socket: &mut TcpStream, patience: Duration) -> bool {
    let mut buf = [0u8; 256];
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        match tokio::time::timeout_at(deadline, socket.read(&mut buf)).await {
            Err(_) => return false,
            Ok(Ok(0) | Err(_)) => return true,
            Ok(Ok(_)) => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_source_over_its_handshake_cap_is_refused_and_the_slots_drain() {
    let Some(ip) = non_loopback_ip() else {
        eprintln!("NOT RUN: this host has no non-loopback address to dial from");
        return;
    };
    let dir = TempDir::new().expect("temp dir");
    let state = Arc::new(StateDB::open(dir.path()).expect("open state"));
    let mut node = Node::new_tcp(state).expect("build node");
    let port = free_tcp_port();
    let listen: Multiaddr = format!("/ip4/0.0.0.0/tcp/{port}").parse().expect("addr");
    node.listen_on(listen).expect("listen");
    let _handle = node.spawn();
    let target = SocketAddr::new(ip, port);

    // Wait for the listener, then hold the source's whole handshake share
    // with sockets that never speak.
    let mut held = Vec::new();
    for _ in 0..50 {
        if let Ok(socket) = TcpStream::connect(target).await {
            held.push(socket);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(held.len(), 1, "the node never listened on {target}");
    while held.len() < MAX_PENDING_PER_SOURCE {
        held.push(TcpStream::connect(target).await.expect("connect"));
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    for socket in &mut held {
        assert!(
            !closed_within(socket, Duration::from_millis(200)).await,
            "a handshake inside the cap was refused"
        );
    }

    // One more from the same source is refused at accept.
    let mut over = TcpStream::connect(target).await.expect("connect");
    assert!(
        closed_within(&mut over, REFUSAL_PATIENCE).await,
        "a handshake over the per-source cap was admitted"
    );

    // The connection timeout ends the stalled handshakes, and their slots
    // come back (ListenFailure frees them), so the source is admitted again.
    tokio::time::sleep(PAST_CONNECTION_TIMEOUT).await;
    for socket in &mut held {
        assert!(
            closed_within(socket, Duration::from_millis(200)).await,
            "a stalled handshake outlived the connection timeout"
        );
    }
    let mut again = TcpStream::connect(target).await.expect("connect");
    assert!(
        !closed_within(&mut again, REFUSAL_PATIENCE).await,
        "the slots of timed-out handshakes were never freed"
    );
}
