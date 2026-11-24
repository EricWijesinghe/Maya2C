//! `get_supply` is the node's costliest public read, so the real node client
//! reuses one answer for `SUPPLY_TTL` whatever route or client asks.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{Json, Router, routing::post};
use maya_api_gateway::node::{NodeClient, RpcNodeClient, SUPPLY_TTL};

/// A JSON-RPC endpoint that answers every call with a supply after `delay`,
/// counting calls as they arrive.
async fn counting_node(delay: std::time::Duration) -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let app = Router::new().route(
        "/",
        post(move |Json(request): Json<serde_json::Value>| {
            let counter = Arc::clone(&counter);
            async move {
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                tokio::time::sleep(delay).await;
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": { "circulating": n.to_string(), "total": "100" },
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, calls)
}

#[tokio::test]
async fn supply_is_fetched_once_per_window() {
    let (url, calls) = counting_node(std::time::Duration::ZERO).await;
    let client = Arc::new(RpcNodeClient::connect(&url).unwrap());

    let concurrent: Vec<_> = (0..8)
        .map(|_| {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.get_supply().await.unwrap() })
        })
        .collect();
    for task in concurrent {
        assert_eq!(task.await.unwrap().circulating, "1");
    }
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "eight callers, one node call"
    );

    tokio::time::sleep(SUPPLY_TTL + std::time::Duration::from_millis(100)).await;
    assert_eq!(client.get_supply().await.unwrap().circulating, "2");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        2,
        "a stale answer is refetched"
    );
}

/// A JSON-RPC endpoint that accepts calls and never answers.
async fn hung_node() -> String {
    let app = Router::new().route("/", post(std::future::pending::<String>));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn a_caller_queued_behind_a_hung_supply_call_gives_up_within_node_timeout() {
    use maya_api_gateway::node::NODE_TIMEOUT;
    use std::time::{Duration, Instant};

    let client = Arc::new(RpcNodeClient::connect(&hung_node().await).unwrap());
    let first = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.get_supply().await })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = Instant::now();
    assert!(
        client.get_supply().await.is_err(),
        "a hung node yields an error"
    );
    let waited = started.elapsed();
    assert!(first.await.unwrap().is_err());
    // Unbounded, the queued caller sits out the first call and then makes
    // its own: two timeouts instead of one.
    assert!(
        waited < NODE_TIMEOUT + NODE_TIMEOUT / 2,
        "queued caller waited {waited:?}, past one NODE_TIMEOUT"
    );
}

#[tokio::test]
async fn a_caller_that_gives_up_mid_call_still_fills_the_cache() {
    use std::time::Duration;

    // A client that disconnects drops its handler future; the node finishes
    // its walk regardless, so the answer must still be kept.
    let (url, calls) = counting_node(Duration::from_millis(300)).await;
    let client = Arc::new(RpcNodeClient::connect(&url).unwrap());
    let abandoned = {
        let client = Arc::clone(&client);
        tokio::spawn(async move { client.get_supply().await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    abandoned.abort();
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(client.get_supply().await.unwrap().circulating, "1");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the abandoned call's answer is reused, not fetched again"
    );
}
