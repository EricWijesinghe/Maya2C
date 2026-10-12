//! `get_peers`, end to end: two real nodes connect over WebSocket (the way a
//! helper's laptop reaches the seed through the public relay), and the seed's
//! RPC lists the joiner as inbound WebSocket, with no address in the answer.
//! Then the joiner leaves and the list empties. This is what the live network
//! dashboard (`apps/status`, `/live`) is built on.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use custom_l1_node::consensus::ChainConfig;
use custom_l1_node::core::{Block, BlockHeader};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::{Mempool, Node, NodeEvent, NodeHandle};
use custom_l1_node::rpc::peers::NetworkView;
use custom_l1_node::rpc::{RpcContext, serve};
use custom_l1_node::state::StateDB;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::HttpClientBuilder;
use jsonrpsee::rpc_params;
use libp2p::Multiaddr;
use tempfile::TempDir;
use tokio::sync::broadcast::error::RecvError;

mod common;

const TIMEOUT: Duration = Duration::from_secs(20);

fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("a free port")
        .port()
}

async fn wait_for(events: &mut tokio::sync::broadcast::Receiver<NodeEvent>, connected: bool) -> bool {
    let wait = async {
        loop {
            match events.recv().await {
                Ok(NodeEvent::PeerConnected(_)) if connected => return true,
                Ok(NodeEvent::PeerDisconnected(_)) if !connected => return true,
                Ok(_) | Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return false,
            }
        }
    };
    tokio::time::timeout(TIMEOUT, wait).await.unwrap_or(false)
}

fn genesis() -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_700_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(1),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

/// An RPC server over `state`, answering `get_peers` from `node`.
async fn rpc_for(state: Arc<StateDB>, node: NodeHandle) -> (custom_l1_node::rpc::RpcServer, jsonrpsee::http_client::HttpClient) {
    let chain = Arc::new(Mutex::new(
        common::open_chain(Arc::clone(&state), genesis(), ChainConfig::without_pow_verification()).unwrap(),
    ));
    let context = RpcContext::new(chain, Mempool::new(state)).with_network_view(node);
    let server = serve("127.0.0.1:0".parse().unwrap(), context).await.unwrap();
    let client = HttpClientBuilder::default()
        .build(format!("http://{}", server.address))
        .unwrap();
    (server, client)
}

#[tokio::test]
async fn the_seed_lists_a_websocket_joiner_without_its_address_and_drops_it_when_it_leaves() {
    let (dir_a, dir_b) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let seed_state = Arc::new(StateDB::open(dir_a.path()).unwrap());
    let mut seed = Node::new_tcp(Arc::clone(&seed_state)).unwrap();
    let joiner = Node::new_tcp(Arc::new(StateDB::open(dir_b.path()).unwrap())).unwrap();

    let ws: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}/ws", free_tcp_port()).parse().unwrap();
    seed.listen_on(ws.clone()).unwrap();
    let seed = seed.spawn();
    let joiner = joiner.spawn();
    let mut seed_events = seed.subscribe();
    tokio::time::sleep(Duration::from_millis(300)).await; // listener up

    let (_server, client) = rpc_for(seed_state, seed.clone()).await;
    let empty: NetworkView = client.request("get_peers", rpc_params![]).await.unwrap();
    assert_eq!(empty.peer_id, seed.peer_id().to_string());
    assert!(empty.peers.is_empty());

    joiner.dial(ws).await.expect("dial over websocket");
    assert!(wait_for(&mut seed_events, true).await, "seed never saw the joiner");

    let view: NetworkView = client.request("get_peers", rpc_params![]).await.unwrap();
    assert_eq!(view.peers.len(), 1, "{view:?}");
    let p = &view.peers[0];
    assert_eq!(p.peer_id, joiner.peer_id().to_string());
    assert_eq!(p.direction, "inbound");
    assert_eq!(p.transport, "websocket");
    assert!(p.connections >= 1);

    // The answer carries no address: not the loopback IP, not the port.
    let raw: serde_json::Value = client.request("get_peers", rpc_params![]).await.unwrap();
    let text = raw.to_string();
    assert!(!text.contains("127.0.0.1"), "address leaked: {text}");

    // The joiner's own view sees the seed as outbound.
    let (_jdir, joiner_state) = {
        let d = TempDir::new().unwrap();
        let s = Arc::new(StateDB::open(d.path()).unwrap());
        (d, s)
    };
    let (_jserver, jclient) = rpc_for(joiner_state, joiner.clone()).await;
    let jview: NetworkView = jclient.request("get_peers", rpc_params![]).await.unwrap();
    assert_eq!(jview.peers.len(), 1);
    assert_eq!(jview.peers[0].direction, "outbound");

    drop(joiner);
    drop(jclient);
    drop(_jserver);
    assert!(wait_for(&mut seed_events, false).await, "seed never saw the joiner leave");
    let after: NetworkView = client.request("get_peers", rpc_params![]).await.unwrap();
    assert!(after.peers.is_empty(), "{after:?}");
}

#[tokio::test]
async fn a_node_without_a_network_view_refuses_rather_than_answering_empty() {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(StateDB::open(dir.path()).unwrap());
    let chain = Arc::new(Mutex::new(
        common::open_chain(Arc::clone(&state), genesis(), ChainConfig::without_pow_verification()).unwrap(),
    ));
    let server = serve("127.0.0.1:0".parse().unwrap(), RpcContext::new(chain, Mempool::new(state)))
        .await
        .unwrap();
    let client = HttpClientBuilder::default()
        .build(format!("http://{}", server.address))
        .unwrap();
    let r: Result<NetworkView, _> = client.request("get_peers", rpc_params![]).await;
    assert!(r.is_err(), "an empty list would read as 'nobody connected'");
}
