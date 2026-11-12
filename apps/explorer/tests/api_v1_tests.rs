//! The public indexer API, v1 (ADR-092), driven through its router.
//!
//! Requests go through `tower::ServiceExt::oneshot`, so no socket is bound and
//! the node is a stand-in that counts calls and fails on demand. History comes
//! from [`MemoryStore`], the same logic the Postgres store is held to by its
//! own contract. The Postgres SQL is NOT covered here: the repository has no
//! PostgreSQL harness.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{HeaderMap, Request, StatusCode};
use custom_l1_node::rpc::types::CheckpointInfo;
use custom_l1_node::rpc::validators::ValidatorsInfo;
use custom_l1_node::rpc::{AccountInfo, ChainInfo, SupplyReport};
use http_body_util::BodyExt;
use maya_address::{Network, encode};
use maya_explorer::api::node::{Mempool, NodeSource};
use maya_explorer::api::{self, ApiState, LiveCache, ROUTES, openapi};
use maya_explorer::error::{ExplorerError, Result};
use maya_explorer::model::{IndexedBlock, IndexedOutput, IndexedTx};
use maya_explorer::server::{AppState, router as explorer_router};
use maya_explorer::store::BlockStore;
use maya_explorer::store::memory::MemoryStore;
use maya_web_security::rate_limit::{ClientIp, RateLimit};
use primitive_types::U256;
use serde_json::Value;
use tokio::sync::broadcast;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

/// A node stand-in: counts `get_supply` calls, can be taken down, and can
/// report a finality checkpoint or a staking chain.
#[derive(Default)]
struct FakeNode {
    supply_calls: AtomicUsize,
    down: AtomicBool,
    finalized: Option<u64>,
    staking: bool,
}

impl FakeNode {
    fn answer(&self) -> Result<()> {
        if self.down.load(Ordering::SeqCst) {
            Err(ExplorerError::NodeRpc("connection refused".to_string()))
        } else {
            Ok(())
        }
    }
}

#[async_trait]
impl NodeSource for FakeNode {
    async fn chain(&self) -> Result<ChainInfo> {
        self.answer()?;
        Ok(ChainInfo {
            genesis: "00".repeat(32),
            chain_id: Some("maya-testnet-1".to_string()),
            height: 5,
        })
    }

    async fn checkpoint(&self) -> Result<Option<CheckpointInfo>> {
        self.answer()?;
        Ok(self.finalized.map(|height| CheckpointInfo {
            epoch: 1,
            height,
            block: hash(height as u8),
            signatures: Vec::new(),
        }))
    }

    async fn account(&self, address_hex: &str) -> Result<AccountInfo> {
        self.answer()?;
        Ok(AccountInfo {
            address: address_hex.to_string(),
            balance: U256::one() << 200,
            nonce: 4,
            address_text: None,
        })
    }

    async fn validators(&self) -> Result<Option<ValidatorsInfo>> {
        self.answer()?;
        Ok(self.staking.then(|| ValidatorsInfo {
            epoch: 1,
            epoch_blocks: 10,
            committee: 0,
            total: 0,
            validators: Vec::new(),
        }))
    }

    async fn supply(&self) -> Result<SupplyReport> {
        self.supply_calls.fetch_add(1, Ordering::SeqCst);
        // Stands in for the node's full account scan.
        tokio::time::sleep(Duration::from_millis(30)).await;
        self.answer()?;
        Ok(SupplyReport {
            total: U256::one() << 100,
            circulating: U256::one() << 99,
            shielded: U256::zero(),
            burned: U256::zero(),
        })
    }

    async fn mempool(&self) -> Result<Mempool> {
        self.answer()?;
        Ok(Mempool {
            size: 2,
            bytes: 512,
        })
    }
}

fn addr(n: u8) -> [u8; 32] {
    [n; 32]
}

fn addr_hex(n: u8) -> String {
    hex::encode(addr(n))
}

/// Bech32m on the testnet, which is the network the fake node serves.
fn bech(n: u8) -> String {
    encode(Network::Testnet, &addr(n))
}

fn hash(n: u8) -> String {
    hex::encode([n; 32])
}

fn txid(height: i64, position: i64) -> String {
    let mut bytes = [0xee_u8; 32];
    bytes[0] = height as u8;
    bytes[1] = position as u8;
    hex::encode(bytes)
}

/// A transaction from `sender`, paying each `(recipient, amount)` in turn.
fn tx(height: i64, position: i64, sender: u8, outputs: &[(u8, &str)]) -> IndexedTx {
    let txid = txid(height, position);
    let outputs: Vec<IndexedOutput> = outputs
        .iter()
        .enumerate()
        .map(|(idx, (to, amount))| IndexedOutput {
            txid: txid.clone(),
            idx: idx as i32,
            recipient: addr_hex(*to),
            amount: (*amount).to_string(),
        })
        .collect();
    IndexedTx {
        txid,
        height,
        sender: addr_hex(sender),
        nonce: position,
        output_count: outputs.len() as i32,
        total_out: "0".to_string(),
        signed: true,
        kind: "transfer".to_string(),
        summary: String::new(),
        careful: false,
        outputs,
    }
}

/// `heights` blocks, each with two transactions:
/// position 0 pays addr 2 (above 2^64) and addr 3; position 1 pays addr 1.
async fn seeded(heights: i64) -> Arc<MemoryStore> {
    let store = Arc::new(MemoryStore::new());
    for height in 1..=heights {
        let txs = vec![
            tx(height, 0, 1, &[(2, "18446744073709551617"), (3, "5")]),
            tx(height, 1, 2, &[(1, "7")]),
        ];
        let block = IndexedBlock {
            height,
            id: hash(height as u8),
            prev_hash: hash(height.saturating_sub(1) as u8),
            state_root: hash(0xab),
            timestamp: 1_700_000_000 + height * 15,
            nonce: height,
            difficulty_target: hash(0xff),
            tx_count: txs.len() as i32,
            work: "1".to_string(),
        };
        store.put_block(&block, &txs).await.expect("seed block");
    }
    store
}

fn limit_of(burst: u32, per_second: u64, client_ip: ClientIp) -> RateLimit {
    RateLimit {
        per_second,
        burst,
        client_ip,
    }
}

fn build_app(store: Arc<MemoryStore>, node: Arc<FakeNode>) -> Router {
    app_with(store, node, limit_of(1_000, 1_000, ClientIp::Socket))
}

fn app_with(store: Arc<MemoryStore>, node: Arc<FakeNode>, limit: RateLimit) -> Router {
    let state = ApiState {
        store,
        node,
        live: Arc::new(LiveCache::default()),
    };
    api::router::<()>(state, &limit).expect("valid limit")
}

fn peer(last: u8) -> SocketAddr {
    SocketAddr::from(([10, 0, 0, last], 4000))
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

impl Reply {
    fn code(&self) -> &str {
        self.body["code"].as_str().unwrap_or("")
    }
}

async fn send(
    app: &Router,
    uri: &str,
    peer: Option<SocketAddr>,
    headers: &[(&str, &str)],
) -> Reply {
    let mut builder = Request::builder().uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let mut request = builder.body(Body::empty()).expect("request");
    if let Some(addr) = peer {
        request.extensions_mut().insert(ConnectInfo(addr));
    }
    let response = app.clone().oneshot(request).await.expect("infallible");
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Reply {
        status,
        headers,
        body,
    }
}

async fn get(app: &Router, uri: &str) -> Reply {
    send(app, uri, Some(peer(1)), &[]).await
}

/// Walks a list to the end with `limit`, returning every item's `txid`.
async fn walk(app: &Router, base: &str, limit: usize) -> Vec<String> {
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let sep = if base.contains('?') { '&' } else { '?' };
        let mut uri = format!("{base}{sep}limit={limit}");
        if let Some(next) = &cursor {
            uri.push_str(&format!("&cursor={next}"));
        }
        let page = get(app, &uri).await;
        assert_eq!(page.status, StatusCode::OK, "{uri}: {}", page.body);
        for item in page.body["items"].as_array().expect("items") {
            seen.push(item["txid"].as_str().expect("txid").to_string());
        }
        match page.body["next_cursor"].as_str() {
            Some(next) => cursor = Some(next.to_string()),
            None => return seen,
        }
    }
}

// ---------------------------------------------------------------------------
// shapes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_reports_chain_and_index() {
    let app = build_app(seeded(5).await, Arc::new(FakeNode::default()));
    let reply = get(&app, "/api/v1/status").await;
    assert_eq!(reply.status, StatusCode::OK);
    let body = &reply.body;
    assert_eq!(body["chain_id"], "maya-testnet-1");
    assert_eq!(body["network"], "tmaya");
    assert_eq!(body["indexed_height"], 5);
    assert_eq!(body["oldest_indexed_height"], 1);
    assert_eq!(body["tip_height"], 5);
    assert_eq!(body["lag_blocks"], 0);
    assert_eq!(body["counts"]["blocks"], 5);
    assert_eq!(body["counts"]["transactions"], 10);
    assert!(body["average_block_time_seconds"].as_f64().unwrap() > 0.0);
    assert_eq!(reply.headers["x-indexed-height"], "5");
    assert_eq!(reply.headers["x-tip-height"], "5");
}

#[tokio::test]
async fn blocks_and_block_lookup_by_height_and_id() {
    let app = build_app(seeded(5).await, Arc::new(FakeNode::default()));
    let page = get(&app, "/api/v1/blocks?limit=2").await;
    assert_eq!(page.body["items"][0]["height"], 5);
    assert_eq!(page.body["items"][1]["height"], 4);

    let by_height = get(&app, "/api/v1/blocks/3").await;
    assert_eq!(by_height.body["id"], hash(3));
    let by_id = get(&app, &format!("/api/v1/blocks/{}", hash(3).to_uppercase())).await;
    assert_eq!(by_id.body["height"], 3);
    assert_eq!(get(&app, "/api/v1/blocks/99").await.code(), "not_found");
}

#[tokio::test]
async fn block_transactions_come_in_inclusion_order() {
    let app = build_app(seeded(2).await, Arc::new(FakeNode::default()));
    let reply = get(&app, "/api/v1/blocks/2/transactions").await;
    assert_eq!(reply.body["items"][0]["position"], 0);
    assert_eq!(reply.body["items"][1]["position"], 1);
    assert_eq!(reply.body["items"][0]["block_height"], 2);
    assert!(reply.body["next_cursor"].is_null());
}

#[tokio::test]
async fn transaction_detail_includes_outputs_with_exact_amounts() {
    let app = build_app(seeded(2).await, Arc::new(FakeNode::default()));
    let reply = get(&app, &format!("/api/v1/transactions/{}", txid(2, 0))).await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.body["sender"], bech(1));
    let outputs = reply.body["outputs"].as_array().expect("outputs");
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0]["recipient"], bech(2));
    // Above 2^64: a JSON number would lose this digit.
    assert_eq!(outputs[0]["amount"], "18446744073709551617");
    assert_eq!(outputs[1]["amount"], "5");
    assert_eq!(
        get(&app, &format!("/api/v1/transactions/{}", hash(9)))
            .await
            .code(),
        "not_found"
    );
    assert_eq!(
        get(&app, "/api/v1/transactions/xyz").await.code(),
        "invalid_param"
    );
}

#[tokio::test]
async fn account_reads_live_balance_and_history_from_index() {
    let app = build_app(seeded(3).await, Arc::new(FakeNode::default()));
    let account = get(&app, &format!("/api/v1/accounts/{}", bech(1))).await;
    assert_eq!(account.status, StatusCode::OK);
    assert_eq!(account.body["nonce"], 4);
    assert_eq!(account.body["balance"], (U256::one() << 200).to_string());
    assert_eq!(account.headers["cache-control"], "no-cache");
    assert!(account.headers.contains_key("etag"));

    let history = get(&app, &format!("/api/v1/accounts/{}/transactions", bech(1))).await;
    assert_eq!(history.body["items"].as_array().unwrap().len(), 6);
}

#[tokio::test]
async fn validators_are_no_staking_on_a_proof_of_work_chain() {
    let app = build_app(seeded(1).await, Arc::new(FakeNode::default()));
    assert_eq!(get(&app, "/api/v1/validators").await.code(), "no_staking");

    let staking = FakeNode {
        staking: true,
        ..FakeNode::default()
    };
    let app = build_app(seeded(1).await, Arc::new(staking));
    assert_eq!(get(&app, "/api/v1/validators").await.status, StatusCode::OK);
}

#[tokio::test]
async fn supply_and_mempool_have_their_documented_fields() {
    let app = build_app(seeded(1).await, Arc::new(FakeNode::default()));
    let supply = get(&app, "/api/v1/supply").await;
    assert_eq!(supply.body["total"], (U256::one() << 100).to_string());
    assert_eq!(supply.body["circulating"], (U256::one() << 99).to_string());

    let mempool = get(&app, "/api/v1/mempool").await;
    assert_eq!(mempool.body, serde_json::json!({"size": 2, "bytes": 512}));
}

// ---------------------------------------------------------------------------
// pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cursor_walk_returns_the_full_set_once() {
    let store = seeded(5).await;
    let expected: Vec<String> = store
        .transactions_page(None, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|(_, tx)| tx.txid)
        .collect();
    assert_eq!(expected.len(), 10);
    let app = build_app(store, Arc::new(FakeNode::default()));
    for limit in [1, 3, 4, 10, 100] {
        assert_eq!(
            walk(&app, "/api/v1/transactions", limit).await,
            expected,
            "limit {limit}"
        );
    }
}

#[tokio::test]
async fn address_walk_covers_both_sides_without_duplicates() {
    let app = build_app(seeded(5).await, Arc::new(FakeNode::default()));
    // Addr 1 sends position 0 of every block and receives position 1: all 10.
    let base = format!("/api/v1/accounts/{}/transactions", bech(1));
    let mut seen = walk(&app, &base, 3).await;
    assert_eq!(seen.len(), 10);
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), 10);
}

#[tokio::test]
async fn rollback_removes_abandoned_rows_from_every_list() {
    let store = seeded(5).await;
    store.rollback_from(4).await.unwrap();
    let app = build_app(store, Arc::new(FakeNode::default()));
    let txs = walk(&app, "/api/v1/transactions", 2).await;
    assert_eq!(txs.len(), 6);
    for abandoned in [txid(4, 0), txid(4, 1), txid(5, 0), txid(5, 1)] {
        assert!(
            !txs.contains(&abandoned),
            "abandoned row {abandoned} listed"
        );
    }
    let heights = get(&app, "/api/v1/blocks?limit=10").await;
    assert_eq!(heights.body["items"][0]["height"], 3);
    let abandoned = get(&app, &format!("/api/v1/transactions/{}", txid(4, 0))).await;
    assert_eq!(abandoned.code(), "not_found");
}

#[tokio::test]
async fn receive_only_account_lists_incoming_transactions() {
    let app = build_app(seeded(3).await, Arc::new(FakeNode::default()));
    let base = format!("/api/v1/accounts/{}/transactions", bech(3));
    assert_eq!(walk(&app, &base, 10).await.len(), 3);
    let received = format!("{base}?direction=received");
    assert_eq!(walk(&app, &received, 10).await.len(), 3);
    let sent = format!("{base}?direction=sent");
    assert!(walk(&app, &sent, 10).await.is_empty());
}

#[tokio::test]
async fn direction_filter_splits_sent_and_received() {
    let app = build_app(seeded(5).await, Arc::new(FakeNode::default()));
    let base = format!("/api/v1/accounts/{}/transactions", bech(2));
    assert_eq!(
        walk(&app, &format!("{base}?direction=sent"), 4).await.len(),
        5
    );
    assert_eq!(
        walk(&app, &format!("{base}?direction=received"), 4)
            .await
            .len(),
        5
    );
    assert_eq!(walk(&app, &base, 4).await.len(), 10);
    let bad = get(&app, &format!("{base}?direction=in")).await;
    assert_eq!(bad.code(), "invalid_param");
}

// ---------------------------------------------------------------------------
// input validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bad_limits_and_cursors_are_typed_errors() {
    let app = build_app(seeded(2).await, Arc::new(FakeNode::default()));
    for limit in ["0", "101", "-1", "many"] {
        let reply = get(&app, &format!("/api/v1/transactions?limit={limit}")).await;
        assert_eq!(reply.status, StatusCode::BAD_REQUEST, "limit {limit}");
        assert_eq!(reply.code(), "invalid_param");
    }
    let garbage = get(&app, "/api/v1/transactions?cursor=nothex").await;
    assert_eq!(garbage.code(), "invalid_cursor");
    let wrong_version = get(
        &app,
        &format!("/api/v1/transactions?cursor=02{}", "00".repeat(16)),
    )
    .await;
    assert_eq!(wrong_version.code(), "invalid_cursor");
}

#[tokio::test]
async fn cursor_from_another_block_is_refused() {
    let app = build_app(seeded(3).await, Arc::new(FakeNode::default()));
    let first = get(&app, "/api/v1/blocks/3/transactions?limit=1").await;
    assert!(!first.body["next_cursor"].is_null());
    let page = get(&app, "/api/v1/transactions?limit=1").await;
    let cursor = page.body["next_cursor"].as_str().unwrap().to_string();
    let reply = get(
        &app,
        &format!("/api/v1/blocks/2/transactions?cursor={cursor}"),
    )
    .await;
    assert_eq!(reply.code(), "invalid_cursor");
}

#[tokio::test]
async fn addresses_must_be_bech32m_for_this_network() {
    let app = build_app(seeded(1).await, Arc::new(FakeNode::default()));
    let hex_form = get(&app, &format!("/api/v1/accounts/{}", addr_hex(1))).await;
    assert_eq!(hex_form.code(), "invalid_address");

    let mut corrupted = bech(1);
    let last = corrupted.pop().unwrap();
    corrupted.push(if last == 'q' { 'p' } else { 'q' });
    assert_eq!(
        get(&app, &format!("/api/v1/accounts/{corrupted}"))
            .await
            .code(),
        "invalid_address"
    );

    let mainnet = encode(Network::Mainnet, &addr(1));
    let other = get(&app, &format!("/api/v1/accounts/{mainnet}")).await;
    assert_eq!(other.status, StatusCode::BAD_REQUEST);
    assert_eq!(other.code(), "wrong_network");
}

// ---------------------------------------------------------------------------
// rate limit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn burst_plus_one_is_limited_with_retry_headers() {
    let limit = limit_of(3, 1, ClientIp::Socket);
    let app = app_with(seeded(1).await, Arc::new(FakeNode::default()), limit);
    for _ in 0..3 {
        assert_eq!(
            send(&app, "/api/v1/mempool", Some(peer(1)), &[])
                .await
                .status,
            StatusCode::OK
        );
    }
    let limited = send(&app, "/api/v1/mempool", Some(peer(1)), &[]).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.code(), "rate_limited");
    assert!(limited.headers.contains_key("retry-after"));
    assert_eq!(limited.headers["ratelimit-limit"], "3");
    assert_eq!(limited.headers["ratelimit-remaining"], "0");
    assert!(limited.headers.contains_key("ratelimit-reset"));
    assert_eq!(limited.headers["content-type"], "application/problem+json");
}

#[tokio::test]
async fn clients_are_limited_independently() {
    let limit = limit_of(2, 1, ClientIp::Socket);
    let app = app_with(seeded(1).await, Arc::new(FakeNode::default()), limit);
    for _ in 0..2 {
        send(&app, "/api/v1/mempool", Some(peer(1)), &[]).await;
    }
    assert_eq!(
        send(&app, "/api/v1/mempool", Some(peer(1)), &[])
            .await
            .status,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        send(&app, "/api/v1/mempool", Some(peer(2)), &[])
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn spoofed_forwarded_for_does_not_rotate_the_key() {
    let limit = limit_of(2, 1, ClientIp::Socket);
    let app = app_with(seeded(1).await, Arc::new(FakeNode::default()), limit);
    for spoof in ["1.1.1.1", "2.2.2.2", "3.3.3.3"] {
        let headers = [("x-forwarded-for", spoof), ("cf-connecting-ip", spoof)];
        let reply = send(&app, "/api/v1/mempool", Some(peer(1)), &headers).await;
        if spoof == "3.3.3.3" {
            assert_eq!(reply.status, StatusCode::TOO_MANY_REQUESTS);
        }
    }
}

#[tokio::test]
async fn proxy_mode_keys_on_the_proxy_appended_address() {
    let limit = limit_of(2, 1, ClientIp::XForwardedForLast);
    let app = app_with(seeded(1).await, Arc::new(FakeNode::default()), limit);
    // The client prepends whatever it likes; the proxy appends the real peer.
    for prefix in ["7.7.7.7", "8.8.8.8", "9.9.9.9"] {
        let value = format!("{prefix}, 203.0.113.9");
        send(
            &app,
            "/api/v1/mempool",
            Some(peer(1)),
            &[("x-forwarded-for", value.as_str())],
        )
        .await;
    }
    let limited = send(
        &app,
        "/api/v1/mempool",
        Some(peer(1)),
        &[("x-forwarded-for", "6.6.6.6, 203.0.113.9")],
    )
    .await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
}

// ---------------------------------------------------------------------------
// node dependence and caching
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fifty_concurrent_supply_requests_make_one_node_call() {
    let node = Arc::new(FakeNode::default());
    let app = app_with(
        seeded(1).await,
        Arc::clone(&node),
        limit_of(1_000, 1_000, ClientIp::Socket),
    );
    let mut tasks = Vec::new();
    for _ in 0..50 {
        let app = app.clone();
        tasks.push(tokio::spawn(async move {
            get(&app, "/api/v1/supply").await.status
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), StatusCode::OK);
    }
    assert_eq!(node.supply_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn node_down_blocks_live_routes_and_leaves_history_working() {
    let node = Arc::new(FakeNode::default());
    node.down.store(true, Ordering::SeqCst);
    let app = build_app(seeded(3).await, node);
    assert_eq!(get(&app, "/api/v1/supply").await.code(), "node_unavailable");
    assert_eq!(
        get(&app, "/api/v1/mempool").await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        get(&app, &format!("/api/v1/accounts/{}", bech(1)))
            .await
            .code(),
        "node_unavailable"
    );
    assert_eq!(get(&app, "/api/v1/blocks/2").await.status, StatusCode::OK);
    let status = get(&app, "/api/v1/status").await;
    assert_eq!(status.status, StatusCode::OK);
    assert!(status.body["tip_height"].is_null());
    assert_eq!(status.body["indexed_height"], 3);
    assert!(!status.headers.contains_key("x-tip-height"));
}

#[tokio::test]
async fn cache_headers_follow_finality() {
    let node = Arc::new(FakeNode {
        finalized: Some(3),
        ..FakeNode::default()
    });
    let app = build_app(seeded(5).await, node);
    let final_block = get(&app, "/api/v1/blocks/2").await;
    assert_eq!(
        final_block.headers["cache-control"],
        "public, max-age=31536000, immutable"
    );
    let tip_block = get(&app, "/api/v1/blocks/4").await;
    assert_eq!(tip_block.headers["cache-control"], "public, max-age=5");
    let list = get(&app, "/api/v1/blocks").await;
    assert_eq!(list.headers["cache-control"], "public, max-age=5");
}

#[tokio::test]
async fn without_a_checkpoint_nothing_is_immutable() {
    let app = build_app(seeded(2).await, Arc::new(FakeNode::default()));
    let block = get(&app, "/api/v1/blocks/1").await;
    assert_eq!(block.headers["cache-control"], "public, max-age=5");
}

#[tokio::test]
async fn live_data_revalidates_with_etag() {
    let app = build_app(seeded(1).await, Arc::new(FakeNode::default()));
    let first = get(&app, "/api/v1/mempool").await;
    let etag = first.headers["etag"].to_str().unwrap().to_string();
    let again = send(
        &app,
        "/api/v1/mempool",
        Some(peer(1)),
        &[("if-none-match", etag.as_str())],
    )
    .await;
    assert_eq!(again.status, StatusCode::NOT_MODIFIED);
    assert_eq!(again.headers["etag"], etag.as_str());
}

// ---------------------------------------------------------------------------
// the spec
// ---------------------------------------------------------------------------

#[test]
fn every_route_is_in_the_spec_and_the_spec_has_no_extra_path() {
    let document = serde_json::to_value(openapi::document()).unwrap();
    let mut documented: Vec<String> = document["paths"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    documented.sort();
    let mut routed: Vec<String> = ROUTES.iter().map(|p| (*p).to_string()).collect();
    routed.sort();
    assert_eq!(documented, routed);
}

#[tokio::test]
async fn every_route_is_served() {
    let app = build_app(seeded(2).await, Arc::new(FakeNode::default()));
    let concrete: Vec<String> = [
        "/api/v1/status".to_string(),
        "/api/v1/blocks".to_string(),
        "/api/v1/blocks/1".to_string(),
        "/api/v1/blocks/1/transactions".to_string(),
        "/api/v1/transactions".to_string(),
        format!("/api/v1/transactions/{}", txid(1, 0)),
        format!("/api/v1/accounts/{}", bech(1)),
        format!("/api/v1/accounts/{}/transactions", bech(1)),
        "/api/v1/validators".to_string(),
        "/api/v1/supply".to_string(),
        "/api/v1/mempool".to_string(),
    ]
    .to_vec();
    assert_eq!(concrete.len(), ROUTES.len(), "one sample URI per route");
    for uri in &concrete {
        let reply = get(&app, uri).await;
        // An unrouted path gets axum's empty 404; ours always answer JSON.
        assert!(!reply.body.is_null(), "{uri} is not routed");
    }
}

#[test]
fn openapi_matches_the_golden_file() {
    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi/v1.yaml");
    let generated = format!(
        "{}\n",
        serde_json::to_string_pretty(&openapi::document()).unwrap()
    );
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &generated).expect("write golden");
    }
    let committed = std::fs::read_to_string(&golden).expect("golden file exists");
    assert_eq!(
        committed, generated,
        "run with UPDATE_GOLDEN=1 after an intended change"
    );
}

#[tokio::test]
async fn legacy_api_is_marked_deprecated_and_v1_is_not() {
    let (events, _) = broadcast::channel(16);
    let state = AppState {
        store: seeded(1).await,
        events,
        node_rpc: None,
        assets: None,
        api_client_ip: ClientIp::Socket,
    };
    let app = explorer_router(state);
    let legacy = get(&app, "/api/stats").await;
    assert_eq!(legacy.headers["deprecation"], "true");
    assert!(
        legacy.headers["link"]
            .to_str()
            .unwrap()
            .contains("successor-version")
    );
    let v1 = get(&app, "/api/v1/status").await;
    assert!(!v1.headers.contains_key("deprecation"));
}
