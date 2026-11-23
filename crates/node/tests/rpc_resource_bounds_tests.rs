//! What one RPC client can make the node spend: request bodies, slow uploads,
//! walked reads and the supply scan.
//!
//! Each test binds a real server on port 0 and talks to it over HTTP, so the
//! bounds are checked where a client meets them, not inside a handler.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use primitive_types::U256;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use custom_l1_node::consensus::{Chain, ChainConfig};
use custom_l1_node::core::{Block, BlockHeader};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::network::Mempool;
use custom_l1_node::rpc::limit::RateLimiter;
use custom_l1_node::rpc::server::{
    BODY_READ_DEADLINE, HEADER_READ_TIMEOUT, MAX_REQUEST_BODY, READ_BUDGET_EXHAUSTED,
};
use custom_l1_node::rpc::{RpcContext, RpcServer, serve};
use custom_l1_node::state::{Account, Address, StateDB};

use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod common;

struct TestNode {
    client: HttpClient,
    chain: Arc<Mutex<Chain>>,
    state: Arc<StateDB>,
    server: RpcServer,
    _dir: TempDir,
}

fn genesis() -> Block {
    Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_700_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(6),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

fn fund(state: &StateDB, address: &Address, balance: u64) {
    state
        .put_account(
            address,
            &Account {
                balance: U256::from(balance),
                nonce: 0,
            },
        )
        .expect("fund");
}

async fn start(funded: &[(Address, u64)], shape: fn(RpcContext) -> RpcContext) -> TestNode {
    let dir = TempDir::new().expect("temp dir");
    let state = Arc::new(StateDB::open(dir.path()).expect("open state"));
    for (address, balance) in funded {
        fund(&state, address, *balance);
    }
    let chain = Arc::new(Mutex::new(
        common::open_chain(
            Arc::clone(&state),
            genesis(),
            ChainConfig::without_pow_verification(),
        )
        .expect("open chain"),
    ));
    let context = shape(RpcContext::new(
        Arc::clone(&chain),
        Mempool::new(Arc::clone(&state)),
    ));
    let server = serve("127.0.0.1:0".parse().expect("addr"), context)
        .await
        .expect("serve");
    let client = HttpClientBuilder::default()
        .build(format!("http://{}", server.address))
        .expect("client");
    TestNode {
        client,
        chain,
        state,
        server,
        _dir: dir,
    }
}

/// Commits an empty block through the chain, as the node does.
fn commit_empty(node: &TestNode) {
    let mut chain = node.chain.lock().expect("chain");
    let timestamp = 1_700_000_000 + chain.height() + 1;
    let block = chain
        .candidate_block(timestamp, Vec::new())
        .expect("candidate");
    chain.insert_block(block).expect("insert");
}

/// Sends `head` and then `body`, and reads whatever the server answers, giving
/// up after `patience`. `None` if the server neither answered nor hung up.
async fn exchange(
    node: &TestNode,
    head: String,
    body: &[u8],
    patience: Duration,
) -> Option<String> {
    let mut stream = tokio::net::TcpStream::connect(node.server.address)
        .await
        .expect("connect");
    stream.write_all(head.as_bytes()).await.expect("head");
    // The server may refuse before reading everything; a broken pipe here is
    // its answer, not a test failure.
    let _ = stream.write_all(body).await;
    let mut answer = Vec::new();
    tokio::time::timeout(patience, stream.read_to_end(&mut answer))
        .await
        .ok()
        .map(|_| String::from_utf8_lossy(&answer).into_owned())
}

fn post_head(length: usize) -> String {
    format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Content-Length: {length}\r\nConnection: close\r\n\r\n"
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_over_the_cap_is_refused_as_too_big() {
    // Under jsonrpsee's 10 MiB default but over the node's own cap: before the
    // cap this body was read whole and dispatched.
    let node = start(&[], |c| c).await;
    let size = MAX_REQUEST_BODY as usize + 1024;
    let prefix = br#"{"jsonrpc":"2.0","id":1,"method":"get_tip_height","params":[""#;
    let suffix = br#""]}"#;
    let mut body = Vec::with_capacity(size);
    body.extend_from_slice(prefix);
    body.resize(size - suffix.len(), b'a');
    body.extend_from_slice(suffix);
    assert!(body.len() < 10 * 1024 * 1024);

    let answer = exchange(&node, post_head(body.len()), &body, Duration::from_secs(60))
        .await
        .expect("an answer");
    assert!(
        answer.contains("-32007"),
        "not refused as too big: {answer:.200}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_body_that_never_arrives_is_cut_off() {
    // Ten bytes of a promised thousand, then silence.
    let node = start(&[], |c| c).await;
    let answer = exchange(
        &node,
        post_head(1_000),
        b"{\"jsonrpc\"",
        BODY_READ_DEADLINE + Duration::from_secs(20),
    )
    .await;
    assert!(answer.is_some(), "the server waited past its body deadline");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn get_supply_is_computed_once_per_tip() {
    let alice = [1u8; 32];
    let node = start(&[(alice, 1_000)], |c| c).await;
    let first: serde_json::Value = node
        .client
        .request("get_supply", rpc_params![])
        .await
        .expect("supply");

    // Written behind the chain's back: the tip has not moved, so the cached
    // report stands and no second account walk ran.
    fund(&node.state, &alice, 5_000);
    let same_tip: serde_json::Value = node
        .client
        .request("get_supply", rpc_params![])
        .await
        .expect("supply");
    assert_eq!(first, same_tip, "a second walk ran at an unchanged tip");

    commit_empty(&node);
    let next_tip: serde_json::Value = node
        .client
        .request("get_supply", rpc_params![])
        .await
        .expect("supply");
    assert_ne!(first, next_tip, "a new tip must be walked again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deep_balance_walk_is_charged_by_the_reads_it_makes() {
    // A budget of 4 reads: depth 0 costs 3 and is served; depth 2 costs 7 and
    // is refused before any read.
    let alice = [2u8; 32];
    let node = start(&[(alice, 1_000)], |c| {
        c.with_read_budget(Arc::new(RateLimiter::new(1, 4)))
    })
    .await;
    commit_empty(&node);
    commit_empty(&node);

    let at = |height: u64| {
        let client = node.client.clone();
        async move {
            client
                .request::<serde_json::Value, _>(
                    "get_balance_at_height",
                    rpc_params![hex::encode(alice), height],
                )
                .await
        }
    };
    assert!(at(2).await.is_ok(), "a walk within the budget is served");
    let refused = at(0).await.expect_err("a walk over the budget is refused");
    assert!(
        refused.to_string().contains(READ_BUDGET_EXHAUSTED),
        "{refused}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_connection_is_closed_after_the_header_deadline() {
    // One socket that sends nothing, one that is answered once and then kept
    // open without another request. Before the deadline the second held its
    // slot for good, and 1024 of them locked every client out.
    let node = start(&[], |c| c).await;
    let patience = HEADER_READ_TIMEOUT + Duration::from_secs(10);

    let silent = tokio::net::TcpStream::connect(node.server.address)
        .await
        .expect("connect");
    let mut kept = tokio::net::TcpStream::connect(node.server.address)
        .await
        .expect("connect");
    let body = br#"{"jsonrpc":"2.0","id":1,"method":"get_chain_info","params":[]}"#;
    let head = format!(
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n",
        body.len()
    );
    kept.write_all(head.as_bytes()).await.expect("head");
    kept.write_all(body).await.expect("body");
    let mut first = [0u8; 64];
    let read = tokio::time::timeout(Duration::from_secs(30), kept.read(&mut first))
        .await
        .expect("an answer")
        .expect("read");
    assert!(
        String::from_utf8_lossy(&first[..read]).starts_with("HTTP/1.1 200"),
        "the request was not served"
    );

    let closed = |mut stream: tokio::net::TcpStream| async move {
        let mut rest = Vec::new();
        tokio::time::timeout(patience, stream.read_to_end(&mut rest))
            .await
            .is_ok()
    };
    let (silent_closed, kept_closed) = tokio::join!(closed(silent), closed(kept));
    assert!(silent_closed, "a socket that never sent a request was kept");
    assert!(kept_closed, "an idle keep-alive socket was kept");
}
