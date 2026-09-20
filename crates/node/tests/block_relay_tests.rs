//! The block relay between two real nodes on loopback.
//!
//! TCP with the full Noise + ML-KEM stack for the connection, UDP for the
//! relay, so the key exchange runs over exactly what it would in production.
//! Checked:
//!
//! - keys are agreed on connection, and a relayed block reaches the gossip
//!   block path and comes out as `BlockReceived` from the right peer;
//! - a relayed body contradicting its own `tx_root` costs the sender the same
//!   offence a gossiped one would;
//! - a node running no relay agrees no keys, and nothing is relayed to it.
//!
//! The XDP receive path is exercised by `hal/ebpf-net/tests/xdp_veth.rs`, which
//! needs Linux and root.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::peer_health::OFFENCES;
use custom_l1_node::network::{Node, NodeEvent, NodeHandle, Offence, RelayConfig};
use custom_l1_node::state::db::StateDB;
use libp2p::{Multiaddr, PeerId};
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

const TIMEOUT: Duration = Duration::from_secs(30);

struct TestNode {
    handle: NodeHandle,
    address: Multiaddr,
    events: broadcast::Receiver<NodeEvent>,
    _dir: TempDir,
}

fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port")
        .port()
}

fn spawn(relay: bool) -> TestNode {
    let dir = TempDir::new().expect("temp dir");
    let state = Arc::new(StateDB::open(dir.path()).expect("open state"));
    let mut node = Node::new_tcp(state).expect("build node");
    if relay {
        let bind = "127.0.0.1:0".parse().expect("address");
        node = node
            .with_block_relay(RelayConfig::socket(bind))
            .expect("enable relay");
    }
    let address: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", free_tcp_port())
        .parse()
        .expect("multiaddr");
    node.listen_on(address.clone()).expect("listen");
    let handle = node.spawn();
    let events = handle.subscribe();
    TestNode {
        handle,
        address,
        events,
        _dir: dir,
    }
}

async fn wait_for<T>(
    events: &mut broadcast::Receiver<NodeEvent>,
    mut pick: impl FnMut(&NodeEvent) -> Option<T>,
) -> Option<T> {
    let search = async {
        loop {
            match events.recv().await {
                Ok(event) => {
                    if let Some(found) = pick(&event) {
                        return Some(found);
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return None,
            }
        }
    };
    tokio::time::timeout(TIMEOUT, search).await.ok().flatten()
}

async fn keyed_pair() -> (TestNode, TestNode) {
    let (mut a, mut b) = (spawn(true), spawn(true));
    b.handle.dial(a.address.clone()).await.expect("dial");
    let (a_id, b_id) = (a.handle.peer_id(), b.handle.peer_id());
    let agreed = |wanted: PeerId| {
        move |event: &NodeEvent| {
            matches!(event, NodeEvent::RelayKeyEstablished(peer) if *peer == wanted).then_some(())
        }
    };
    assert!(
        wait_for(&mut a.events, agreed(b_id)).await.is_some(),
        "a never agreed keys"
    );
    assert!(
        wait_for(&mut b.events, agreed(a_id)).await.is_some(),
        "b never agreed keys"
    );
    (a, b)
}

fn block(nonce: u64) -> Block {
    let header = BlockHeader {
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp: 1,
        nonce,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    Block::new(header, vec![])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relayed_block_reaches_the_gossip_block_path_from_the_right_peer() {
    let (a, mut b) = keyed_pair().await;
    let sent = block(7);
    let id = sent.header.id();

    assert_eq!(a.handle.relay_block(&sent).await.expect("relay"), 1);

    let a_id = a.handle.peer_id();
    let relayed = wait_for(&mut b.events, |event| match event {
        NodeEvent::BlockRelayed { peer, block_id } => Some((*peer, *block_id)),
        _ => None,
    })
    .await;
    assert_eq!(relayed, Some((a_id, id)));
    let received = wait_for(&mut b.events, |event| match event {
        NodeEvent::BlockReceived { block, source, .. } => Some((block.header.id(), *source)),
        _ => None,
    })
    .await;
    assert_eq!(received, Some((id, a_id)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relayed_body_contradicting_its_tx_root_costs_the_sender() {
    let (a, mut b) = keyed_pair().await;
    let mut lie = block(8);
    lie.header.tx_root = [9; 32];

    assert_eq!(a.handle.relay_block(&lie).await.expect("relay"), 1);
    assert!(
        wait_for(&mut b.events, |event| matches!(
            event,
            NodeEvent::BlockRelayed { .. }
        )
        .then_some(()))
        .await
        .is_some(),
        "the body never arrived"
    );

    let slot = OFFENCES
        .iter()
        .position(|offence| *offence == Offence::TxRootMismatch)
        .expect("a scored offence");
    let a_id = a.handle.peer_id();
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let report = b.handle.peer_report(a_id).await.expect("report");
        if report.is_some_and(|r| r.offences[slot] >= 1) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the offence was never recorded"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_node_without_a_relay_agrees_no_keys_and_receives_nothing_by_relay() {
    let (mut a, b) = (spawn(true), spawn(false));
    b.handle.dial(a.address.clone()).await.expect("dial");
    let b_id = b.handle.peer_id();
    assert!(
        wait_for(&mut a.events, |event| {
            matches!(event, NodeEvent::PeerConnected(peer) if *peer == b_id).then_some(())
        })
        .await
        .is_some(),
        "never connected"
    );
    // Long enough for an exchange over loopback to have finished several times.
    let agreed = tokio::time::timeout(
        Duration::from_secs(2),
        wait_for(&mut a.events, |event| {
            matches!(event, NodeEvent::RelayKeyEstablished(_)).then_some(())
        }),
    )
    .await;
    assert!(agreed.is_err() || agreed.is_ok_and(|found| found.is_none()));
    assert_eq!(a.handle.relay_block(&block(9)).await.expect("relay"), 0);
}
