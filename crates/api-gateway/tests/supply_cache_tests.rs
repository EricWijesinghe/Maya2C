//! `get_supply` is the node's costliest public read, so the real node client
//! reuses one answer for `SUPPLY_TTL` whatever route or client asks.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::{Json, Router, routing::post};
use maya_api_gateway::node::{NodeClient, RpcNodeClient, SUPPLY_TTL};

/// A JSON-RPC endpoint that answers every call with a supply and counts them.
async fn counting_node() -> (String, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let app = Router::new().route(
        "/",
        post(move |Json(request): Json<serde_json::Value>| {
            let counter = Arc::clone(&counter);
            async move {
                let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
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
    let (url, calls) = counting_node().await;
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
    assert_eq!(calls.load(Ordering::SeqCst), 1, "eight callers, one node call");

    tokio::time::sleep(SUPPLY_TTL + std::time::Duration::from_millis(100)).await;
    assert_eq!(client.get_supply().await.unwrap().circulating, "2");
    assert_eq!(calls.load(Ordering::SeqCst), 2, "a stale answer is refetched");
}
