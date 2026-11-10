//! The explorer served over real HTTP.
//!
//! Binds an actual socket on port 0 and drives it with real requests — the same
//! approach the node's RPC tests take. Nothing is stubbed below the router, so
//! a passing test here means the pages genuinely render and the WebSocket
//! genuinely pushes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

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
        total_out: (500 + index as i64).to_string(),
        signed: true,
        kind: String::new(),
        summary: String::new(),
        careful: false,
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
        // The real directory, resolved from the manifest rather than the
        // working directory, so the asset routes are exercised as deployed
        // rather than skipped. `cargo test` and `cargo nextest` do not agree
        // on the cwd of a test binary; `CARGO_MANIFEST_DIR` is fixed.
        assets: Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")),
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
    // The brand is an image now, so this passes on its `alt` text. That is not
    // a loophole — an image that is the entire content of the home link needs
    // alt text or the link announces itself as unlabelled.
    assert!(body.contains("Maya2C"));
    assert!(body.contains("Block time"));
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
    assert!(body.contains("Block time"));
    assert!(
        body.contains("Blocks / min"),
        "an empty index still renders its tiles"
    );
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

// ---------------------------------------------------------------------------
// branding assets
// ---------------------------------------------------------------------------
//
// The explorer builds its HTML from a Rust string, so unlike the trunk-built
// frontends nothing checks its asset references at build time. These tests are
// that check: they assert the paths the pages emit are paths the server serves.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_asset_the_page_references_is_served() {
    // Parsed out of the rendered page rather than listed by hand, so adding a
    // reference to `ui.rs` without deploying the file fails here instead of in
    // somebody's browser console.
    let h = harness(1).await;
    let (_, body) = get(&format!("{}/", h.base)).await;

    let mut checked = 0;
    for value in body.split('"') {
        // A `srcset` is a comma-separated list of `url descriptor` pairs, not a
        // single URL, so every attribute value is split the same way and the
        // descriptor dropped. The first draft of this test treated the whole
        // srcset as one path and failed with a 400 — which is the test working.
        for candidate in value.split(',') {
            let path = candidate.split_whitespace().next().unwrap_or("");
            if !path.starts_with("/assets/") && path != "/favicon.ico" {
                continue;
            }
            let (status, _) = get(&format!("{}{path}", h.base)).await;
            assert_eq!(status, 200, "{path} is referenced but not served");
            checked += 1;
        }
    }

    assert!(
        checked >= 8,
        "expected the favicon set, manifest, logo and social card; found {checked}"
    );
}

#[tokio::test]
async fn every_response_sends_the_hardening_headers_and_no_server_name() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let h = harness(1).await;
    let authority = h.base.trim_start_matches("http://").to_owned();
    for path in ["/", "/health", "/api/stats", "/no-such-route"] {
        let mut stream = tokio::net::TcpStream::connect(&authority)
            .await
            .expect("connect");
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.expect("write");
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.expect("read");
        // Lowercased, so the header names and the values compare exactly.
        let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
        let head = text.split("\r\n\r\n").next().unwrap_or_default();
        for expected in [
            "\r\nx-content-type-options: nosniff\r\n",
            "\r\nx-frame-options: deny\r\n",
            "\r\nreferrer-policy: no-referrer\r\n",
            "\r\nserver: maya2c\r\n",
            "\r\ncontent-security-policy: ",
        ] {
            assert!(
                head.contains(expected),
                "{path} is missing {expected:?}:\n{head}"
            );
        }
    }
}

#[tokio::test]
async fn security_txt_is_served_at_the_well_known_path() {
    let h = harness(1).await;
    let (status, body) = get(&format!("{}/.well-known/security.txt", h.base)).await;
    assert_eq!(status, 200);
    assert!(body.contains("Contact: https://"), "{body}");
    assert!(body.contains("Expires: "), "{body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_bare_favicon_path_is_served() {
    // Browsers request `/favicon.ico` on their own, whatever the `<link>` tags
    // say, so it is routed at the root as well as under `/assets`.
    let h = harness(1).await;
    let (status, _) = get(&format!("{}/favicon.ico", h.base)).await;
    assert_eq!(status, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deployment_without_an_assets_directory_still_serves_pages() {
    // `assets: None` is what the binary sets when the directory is missing. The
    // icons 404, which is unavoidable, but the explorer itself must keep
    // working — a missing logo is not a reason for the chain explorer to be
    // down.
    let store = Arc::new(MemoryStore::new());
    let (events, _) = broadcast::channel(64);
    let server = serve(
        "127.0.0.1:0".parse().expect("addr"),
        AppState {
            store: store as Arc<dyn BlockStore>,
            events,
            node_rpc: None,
            assets: None,
        },
    )
    .await
    .expect("serve");

    let base = format!("http://{}", server.address);
    let (status, body) = get(&format!("{base}/")).await;
    assert_eq!(status, 200);
    assert!(body.contains("Block time"));

    let (status, _) = get(&format!("{base}/favicon.ico")).await;
    assert_eq!(
        status, 404,
        "no route is mounted when there is nothing to serve"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_public_api_is_readable_from_the_website() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let h = harness(3).await;
    let authority = h.base.trim_start_matches("http://").to_owned();
    let mut stream = tokio::net::TcpStream::connect(&authority)
        .await
        .expect("connect");
    let request = format!(
        "GET /api/stats HTTP/1.1
Host: {authority}
Origin: https://maya2c.dev
Connection: close

"
    );
    stream.write_all(request.as_bytes()).await.expect("write");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.expect("read");
    let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
    assert!(text.starts_with("http/1.1 200"), "{text}");
    assert!(text.contains("access-control-allow-origin: *"), "{text}");
}

/// Seeds block `height` with one transaction of the given kind, then returns it.
async fn seed_kinded_tx(h: &Harness, height: i64, kind: &str, summary: &str) -> IndexedTx {
    let tx = IndexedTx {
        kind: kind.to_owned(),
        summary: summary.to_owned(),
        careful: true,
        ..transaction(height, 9)
    };
    h.store
        .put_block(&block(height, 1), std::slice::from_ref(&tx))
        .await
        .expect("seed kinded");
    tx
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_transaction_api_reports_kind_summary_and_care() {
    let h = harness(3).await;
    let tx = seed_kinded_tx(&h, 1, "place_order", "Place an order to buy 5 units.").await;

    let (status, body) = get(&format!("{}/api/tx/{}", h.base, tx.txid)).await;

    assert_eq!(status, 200);
    let found: IndexedTx = serde_json::from_str(&body).expect("json");
    assert_eq!(found.kind, "place_order");
    assert_eq!(found.summary, "Place an order to buy 5 units.");
    assert!(found.careful);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_block_transactions_api_lists_each_kind() {
    let h = harness(3).await;
    seed_kinded_tx(&h, 1, "cancel_order", "Cancel an open order.").await;

    let (status, body) = get(&format!("{}/api/blocks/1/transactions", h.base)).await;

    assert_eq!(status, 200);
    let txs: Vec<IndexedTx> = serde_json::from_str(&body).expect("json");
    assert_eq!(txs.len(), 1);
    assert_eq!(txs[0].kind, "cancel_order");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_block_transactions_api_404s_for_a_missing_block() {
    let h = harness(2).await;
    let (status, body) = get(&format!("{}/api/blocks/77/transactions", h.base)).await;

    assert_eq!(status, 404);
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert!(value["error"].as_str().unwrap_or_default().contains("77"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_block_page_shows_kind_with_its_summary_as_tooltip() {
    let h = harness(3).await;
    seed_kinded_tx(&h, 1, "place_order", "Place an order to buy 5 units.").await;

    let (status, body) = get(&format!("{}/blocks/1", h.base)).await;

    assert_eq!(status, 200);
    assert!(body.contains("place_order (check first)"), "{body}");
    assert!(
        body.contains(r#"title="Place an order to buy 5 units.""#),
        "{body}"
    );
}

// ---------------------------------------------------------------------------
// explorer v2: mining page, charts, Bech32m account paths, theme toggle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_mining_page_draws_the_hashrate_series() {
    let h = harness(12).await;

    let (status, body) = get(&format!("{}/mining", h.base)).await;

    assert_eq!(status, 200);
    assert!(body.contains("Hashrate over recent blocks"), "{body}");
    assert!(body.contains("<polyline"), "{body}");
    assert!(body.contains("<svg"), "{body}");
}

#[tokio::test]
async fn the_mining_page_renders_on_an_empty_index() {
    let h = harness(0).await;

    let (status, body) = get(&format!("{}/mining", h.base)).await;

    assert_eq!(status, 200);
    assert!(body.contains("No data"), "{body}");
}

#[tokio::test]
async fn the_dashboard_charts_block_time_and_transactions_as_svg() {
    let h = harness(10).await;

    let (status, body) = get(&format!("{}/", h.base)).await;

    assert_eq!(status, 200);
    assert!(body.contains("Block time"), "{body}");
    assert!(body.contains("Transactions per block"), "{body}");
    assert!(body.matches("<svg").count() >= 2, "{body}");
    // Charts are inline SVG; no script library is loaded for them.
    assert!(!body.contains("chart.js"), "{body}");
}

#[tokio::test]
async fn the_nav_links_the_mining_page_and_offers_a_theme_toggle() {
    let h = harness(1).await;

    let (_, body) = get(&format!("{}/", h.base)).await;

    assert!(body.contains(r#"href="/mining""#), "{body}");
    assert!(body.contains("data-theme-toggle"), "{body}");
}

#[tokio::test]
async fn an_account_path_accepts_a_bech32m_address_and_shows_its_text() {
    let h = harness(2).await;
    let id = [0xaa_u8; 32];
    let text = maya_address::encode(maya_address::Network::Testnet, &id);

    let (status, body) = get(&format!("{}/account/{text}", h.base)).await;

    assert_eq!(status, 200);
    assert!(body.contains(&text), "{body}");
    assert!(body.contains(&"aa".repeat(32)), "{body}");
}

#[tokio::test]
async fn a_bech32m_address_typed_in_the_query_is_accepted() {
    let h = harness(2).await;
    let text = maya_address::encode(maya_address::Network::Testnet, &[0xbb_u8; 32]);

    let (status, body) = get(&format!("{}/account?address={text}", h.base)).await;

    assert_eq!(status, 200);
    assert!(body.contains(&text), "{body}");
}

#[tokio::test]
async fn a_hex_address_still_resolves_and_lists_its_transactions() {
    let h = harness(3).await;

    let (status, body) = get(&format!("{}/account?address={}", h.base, "aa".repeat(32))).await;

    assert_eq!(status, 200);
    assert!(body.contains("Transactions sent"), "{body}");
    // No node is configured, so the network is unknown and no text form is shown.
    assert!(!body.contains("tmaya1"), "{body}");
}

#[tokio::test]
async fn a_corrupt_bech32m_address_is_rejected_with_a_reason() {
    let h = harness(1).await;
    let text = maya_address::encode(maya_address::Network::Testnet, &[0xcc_u8; 32]);
    // Flip the last character: the checksum no longer matches.
    let mut bad = text.clone();
    let last = bad.pop().expect("non-empty");
    bad.push(if last == 'q' { 'p' } else { 'q' });

    let (status, body) = get(&format!("{}/account/{bad}", h.base)).await;

    assert_eq!(status, 400);
    assert!(body.contains("Bech32m address"), "{body}");
}
