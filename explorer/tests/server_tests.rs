//! The explorer served over real HTTP.
//!
//! Binds an actual socket on port 0 and drives it with real requests — the same
//! approach the node's RPC tests take. Nothing is stubbed below the router, so
//! a passing test here means the pages genuinely render and the WebSocket
//! genuinely pushes.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use maya_explorer::indexer::IndexEvent;
use maya_explorer::model::{IndexedBlock, IndexedTx, NetworkStats};
use maya_explorer::server::{AppState, ExplorerServer, serve};
use maya_explorer::store::BlockStore;
use maya_explorer::store::memory::MemoryStore;
use tokio::sync::broadcast;

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

struct Harness {
    base: String,
    store: Arc<MemoryStore>,
    events: broadcast::Sender<IndexEvent>,
    _server: ExplorerServer,
}

fn block(height: i64, tx_count: i32) -> IndexedBlock {
    IndexedBlock {
        height,
        id: format!("{height:064x}"),
        prev_hash: format!("{:064x}", height.saturating_sub(1)),
        state_root: "ab".repeat(32),
        // 15s apart, so hashrate and block-time arithmetic have real intervals.
        timestamp: 1_700_000_000 + height * 15,
        nonce: height * 7,
        difficulty_target: format!("00{}", "ff".repeat(31)),
        tx_count,
        work: "256".to_string(),
    }
}

fn transaction(height: i64, index: usize) -> IndexedTx {
    IndexedTx {
        txid: format!("{height:032x}{index:032x}"),
        height,
        sender: "aa".repeat(32),
        nonce: index as i64,
        output_count: 1,
        total_out: 500 + index as i64,
        signed: true,
    }
}

/// Starts a server pre-populated with `blocks` heights.
async fn harness(blocks: i64) -> Harness {
    let store = Arc::new(MemoryStore::new());

    for height in 0..blocks {
        let transactions: Vec<IndexedTx> = (0..2).map(|i| transaction(height, i)).collect();
        store
            .put_block(&block(height, 2), &transactions)
            .await
            .expect("seed");
    }

    let (events, _) = broadcast::channel(64);
    let state = AppState {
        store: Arc::clone(&store) as Arc<dyn BlockStore>,
        events: events.clone(),
        node_rpc: None,
    };

    // Port 0: the OS assigns one, so concurrent tests never collide.
    let server = serve("127.0.0.1:0".parse().expect("addr"), state)
        .await
        .expect("serve");

    Harness {
        base: format!("http://{}", server.address),
        store,
        events,
        _server: server,
    }
}

/// Minimal HTTP GET, so the test suite needs no HTTP client dependency.
async fn get(url: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let without_scheme = url.trim_start_matches("http://");
    let (authority, path) = match without_scheme.find('/') {
        Some(index) => without_scheme.split_at(index),
        None => (without_scheme, "/"),
    };

    let mut stream = tokio::net::TcpStream::connect(authority)
        .await
        .expect("connect");
    let request = format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.expect("write");

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let text = String::from_utf8_lossy(&raw).to_string();

    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let body = text
        .split_once("\r\n\r\n")
        .map_or("", |(_, b)| b)
        .to_string();

    (status, body)
}

// ---------------------------------------------------------------------------
// pages
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_dashboard_renders_with_live_figures() {
    let h = harness(10).await;
    let (status, body) = get(&format!("{}/", h.base)).await;

    assert_eq!(status, 200);
    assert!(body.contains("Maya2C"));
    assert!(body.contains("Hash rate"));
    // Height 9 is the tip of ten blocks.
    assert!(body.contains("stat-height"));
    assert!(body.contains(">9<"), "the tip height is not rendered");
    // The live client must be present, or the page is not actually live.
    assert!(body.contains("/ws/blocks"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_dashboard_renders_with_an_empty_index() {
    // A freshly started explorer serves a page rather than erroring.
    let h = harness(0).await;
    let (status, body) = get(&format!("{}/", h.base)).await;

    assert_eq!(status, 200);
    assert!(body.contains("Hash rate"));
    assert!(body.contains("0 H/s"), "an empty index should read zero");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_block_list_renders() {
    let h = harness(5).await;
    let (status, body) = get(&format!("{}/blocks", h.base)).await;

    assert_eq!(status, 200);
    assert!(body.contains("Latest blocks"));
    assert!(body.contains("/blocks/4"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_block_page_shows_its_transactions() {
    let h = harness(3).await;
    let (status, body) = get(&format!("{}/blocks/1", h.base)).await;

    assert_eq!(status, 200);
    assert!(body.contains("Block 1"));
    assert!(body.contains("State root"));
    assert!(body.contains("2 transaction(s)"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_block_returns_404_with_a_page() {
    let h = harness(3).await;
    let (status, body) = get(&format!("{}/blocks/999", h.base)).await;

    // A 404 that still renders chrome beats a bare error string.
    assert_eq!(status, 404);
    assert!(body.contains("Not found"));
    assert!(body.contains("999"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_transaction_inspector_finds_a_transaction() {
    let h = harness(2).await;
    let txid = transaction(1, 0).txid;

    let (status, body) = get(&format!("{}/tx/{txid}", h.base)).await;
    assert_eq!(status, 200);
    assert!(body.contains(&txid));
    assert!(body.contains("Sender"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_transaction_inspector_reports_an_unknown_id() {
    let h = harness(2).await;
    let (status, body) = get(&format!("{}/tx/{}", h.base, "ff".repeat(32))).await;

    assert_eq!(status, 404);
    assert!(body.contains("No transaction found"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_empty_inspector_prompts_rather_than_erroring() {
    let h = harness(2).await;
    let (status, body) = get(&format!("{}/tx", h.base)).await;

    assert_eq!(status, 200);
    assert!(body.contains("Enter a transaction id"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_lookup_lists_transactions_sent() {
    let h = harness(3).await;
    let address = "aa".repeat(32);

    let (status, body) = get(&format!("{}/account?address={address}", h.base)).await;
    assert_eq!(status, 200);
    assert!(body.contains("Transactions sent"));
    // No node is configured in the harness, so balances must degrade to a
    // message rather than a wrong number.
    assert!(body.contains("balances unavailable"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_malformed_address_is_rejected_before_any_lookup() {
    let h = harness(1).await;
    let (status, body) = get(&format!("{}/account?address=nothex", h.base)).await;

    assert_eq!(status, 400);
    assert!(body.contains("64 hexadecimal"));
}

// ---------------------------------------------------------------------------
// JSON API
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stats_endpoint_reports_the_index() {
    let h = harness(8).await;
    let (status, body) = get(&format!("{}/api/stats", h.base)).await;

    assert_eq!(status, 200);
    let stats: NetworkStats = serde_json::from_str(&body).expect("json");
    assert_eq!(stats.height, 7);
    assert_eq!(stats.blocks_indexed, 8);
    assert_eq!(stats.transactions_indexed, 16);
    // Blocks 15s apart with 256 work each.
    assert!(stats.hashrate > 0.0);
    assert!((stats.average_block_time - 15.0).abs() < 0.001);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_blocks_endpoint_returns_newest_first() {
    let h = harness(5).await;
    let (status, body) = get(&format!("{}/api/blocks", h.base)).await;

    assert_eq!(status, 200);
    let blocks: Vec<IndexedBlock> = serde_json::from_str(&body).expect("json");
    assert_eq!(blocks[0].height, 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_api_reports_a_missing_block_as_json() {
    let h = harness(2).await;
    let (status, body) = get(&format!("{}/api/blocks/42", h.base)).await;

    assert_eq!(status, 404);
    // An API caller gets JSON, not an HTML page.
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(value["error"].as_str().unwrap_or_default().contains("42"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hashrate_series_has_a_point_per_window() {
    let h = harness(12).await;
    let (status, body) = get(&format!("{}/api/hashrate", h.base)).await;

    assert_eq!(status, 200);
    let points: Vec<maya_explorer::model::HashratePoint> =
        serde_json::from_str(&body).expect("json");
    // Twelve blocks over a five-block sub-window.
    assert_eq!(points.len(), 8);
    assert!(points.iter().all(|p| p.hashrate.is_finite()));
    // Oldest first, so the chart reads left to right.
    assert!(points[0].height < points[points.len() - 1].height);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn health_reports_ok() {
    let h = harness(0).await;
    let (status, body) = get(&format!("{}/health", h.base)).await;
    assert_eq!(status, 200);
    assert_eq!(body.trim(), "ok");
}

// ---------------------------------------------------------------------------
// WebSocket
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_block_socket_pushes_newly_indexed_blocks() {
    let h = harness(1).await;
    let url = format!("ws://{}/ws/blocks", h.base.trim_start_matches("http://"));

    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("websocket connect");

    // Publish after connecting: a broadcast receiver only sees what follows it.
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.events
        .send(IndexEvent::Block(Box::new(block(42, 0))))
        .expect("publish");

    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("no message within the timeout")
        .expect("stream ended")
        .expect("message");

    let text = message.into_text().expect("text frame");
    let pushed: IndexedBlock = serde_json::from_str(&text).expect("json");
    assert_eq!(pushed.height, 42);

    let _ = socket.close(None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_transaction_socket_only_receives_transactions() {
    let h = harness(1).await;
    let url = format!("ws://{}/ws/txs", h.base.trim_start_matches("http://"));

    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("websocket connect");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // A block first: the transaction feed must not deliver it.
    h.events
        .send(IndexEvent::Block(Box::new(block(99, 0))))
        .expect("publish block");
    h.events
        .send(IndexEvent::Transaction(Box::new(transaction(99, 3))))
        .expect("publish tx");

    let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("no message within the timeout")
        .expect("stream ended")
        .expect("message");

    let text = message.into_text().expect("text frame");
    let pushed: IndexedTx = serde_json::from_str(&text).expect("json");
    assert_eq!(pushed.height, 99);
    assert_eq!(pushed.nonce, 3);

    let _ = socket.close(None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rollback_reaches_both_feeds() {
    let h = harness(1).await;

    let (mut blocks, _) = tokio_tungstenite::connect_async(&format!(
        "ws://{}/ws/blocks",
        h.base.trim_start_matches("http://")
    ))
    .await
    .expect("connect blocks");
    let (mut txs, _) = tokio_tungstenite::connect_async(&format!(
        "ws://{}/ws/txs",
        h.base.trim_start_matches("http://")
    ))
    .await
    .expect("connect txs");

    tokio::time::sleep(Duration::from_millis(50)).await;
    h.events
        .send(IndexEvent::Rollback {
            from_height: 5,
            removed: 3,
        })
        .expect("publish");

    // A client showing an abandoned block needs to hear about it regardless of
    // which feed it subscribed to.
    for socket in [&mut blocks, &mut txs] {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("no message within the timeout")
            .expect("stream ended")
            .expect("message");
        let text = message.into_text().expect("text frame");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["rollback"]["from_height"], 5);
        assert_eq!(value["rollback"]["removed"], 3);
    }

    let _ = blocks.close(None).await;
    let _ = txs.close(None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_clients_all_receive_the_same_block() {
    let h = harness(1).await;
    let url = format!("ws://{}/ws/blocks", h.base.trim_start_matches("http://"));

    let mut sockets = Vec::new();
    for _ in 0..5 {
        let (socket, _) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("connect");
        sockets.push(socket);
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    h.events
        .send(IndexEvent::Block(Box::new(block(7, 0))))
        .expect("publish");

    for socket in &mut sockets {
        let message = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .expect("no message within the timeout")
            .expect("stream ended")
            .expect("message");
        let pushed: IndexedBlock =
            serde_json::from_str(&message.into_text().expect("text")).expect("json");
        assert_eq!(pushed.height, 7);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn indexing_continues_after_a_client_disconnects() {
    let h = harness(1).await;
    let url = format!("ws://{}/ws/blocks", h.base.trim_start_matches("http://"));

    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = socket.close(None).await;
    drop(socket);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Publishing to a channel whose last reader vanished must not fail the
    // indexer — a browser closing a tab cannot be allowed to stop indexing.
    let _ = h.events.send(IndexEvent::Block(Box::new(block(1, 0))));

    h.store
        .put_block(&block(1, 0), &[])
        .await
        .expect("indexing continues");
    assert_eq!(h.store.latest_height().await.expect("tip"), Some(1));
}
