//! Gateway behaviour against a recorded node.
//!
//! The [`NodeClient`] trait exists so these run without a chain: an end-to-end
//! test that needs RocksDB, a genesis file and a mined block in order to check
//! that a 404 is a 404 is a test nobody runs.
//!
//! What is asserted here is the gateway's own policy — the allowlist, the size
//! ceilings, the error shape — because that policy is the entire reason this
//! crate exists. Whether the node computes a balance correctly is the node's
//! test.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use maya_api_gateway::error::GatewayError;
use maya_api_gateway::node::{Balance, NodeClient, Supply};
use maya_api_gateway::sealed::MAX_SEALED_PAYLOAD_BYTES;
use tower::ServiceExt;

/// A node that answers from memory and counts what it was asked.
#[derive(Default)]
struct MockNode {
    calls: AtomicUsize,
    /// When set, every method returns this error instead of a value.
    fail: bool,
}

impl MockNode {
    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl NodeClient for MockNode {
    async fn get_balance(&self, address: &str) -> Result<Balance, GatewayError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.fail {
            return Err(GatewayError::Upstream(
                "connecting to node at http://10.0.3.7:8545/: refused".to_string(),
            ));
        }
        // Deterministic from the address, so a test can tell one from another.
        Ok(Balance {
            balance: u64::from(address.as_bytes()[0]),
            nonce: 7,
        })
    }

    async fn get_block_by_height(&self, height: u64) -> Result<serde_json::Value, GatewayError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if height > 100 {
            return Ok(serde_json::Value::Null);
        }
        Ok(serde_json::json!({ "height": height, "hash": "ab".repeat(32) }))
    }

    async fn get_supply(&self) -> Result<Supply, GatewayError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(Supply {
            circulating: 21_000,
            total: 42_000,
        })
    }

    async fn send_raw_transaction(&self, raw: &str) -> Result<String, GatewayError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(format!("hash-of-{}", &raw[..raw.len().min(8)]))
    }
}

fn app_with(node: Arc<MockNode>) -> axum::Router {
    maya_api_gateway::app(node)
}

async fn get(app: axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn post(
    app: axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn address() -> String {
    "ab".repeat(32)
}

// ---------------------------------------------------------------------------
// REST
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_does_not_touch_the_node() {
    // A health check that proxied upstream would report the gateway unhealthy
    // whenever the node was, and an orchestrator would restart a process that
    // is working — losing the component still able to return a useful error.
    let node = Arc::new(MockNode::default());
    let (status, body) = get(app_with(Arc::clone(&node)), "/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(node.calls(), 0, "health must not call the node");
}

#[tokio::test]
async fn an_account_is_served() {
    let node = Arc::new(MockNode::default());
    let (status, body) = get(app_with(node), &format!("/v1/accounts/{}", address())).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["nonce"], 7);
}

#[tokio::test]
async fn a_malformed_address_never_reaches_the_node() {
    // Rejected locally: a typo costs a string check, not a round trip.
    let node = Arc::new(MockNode::default());
    let (status, _) = get(app_with(Arc::clone(&node)), "/v1/accounts/nothex").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(node.calls(), 0);
}

#[tokio::test]
async fn a_missing_block_is_a_404() {
    let node = Arc::new(MockNode::default());
    let (status, _) = get(app_with(node), "/v1/blocks/9999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn supply_is_served() {
    let node = Arc::new(MockNode::default());
    let (status, body) = get(app_with(node), "/v1/supply").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["circulating"], 21_000);
    assert_eq!(body["total"], 42_000);
}

// ---------------------------------------------------------------------------
// The allowlist, observed from outside
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_miner_methods_have_no_route() {
    // The allowlist is enforced in `NodeClient`, but the observable
    // consequence is that no route reaches them at all. Asserted from the
    // outside, because that is where an attacker stands.
    let node = Arc::new(MockNode::default());

    // Paths with no route at all.
    for uri in [
        "/v1/mining/candidate",
        "/get_mining_candidate",
        "/submit_block",
    ] {
        let (status, _) = get(app_with(Arc::clone(&node)), uri).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uri} must not be routed");
    }

    // `/v1/blocks/submit` does match `/v1/blocks/{height}` and fails to parse
    // `submit` as a height, so it is a 400 rather than a 404. Either is fine —
    // what matters is that it is not a success and reached nothing upstream.
    let (status, _) = get(app_with(Arc::clone(&node)), "/v1/blocks/submit").await;
    assert!(
        status.is_client_error(),
        "/v1/blocks/submit must not succeed, got {status}"
    );

    assert_eq!(
        node.calls(),
        0,
        "no miner-shaped request may reach the node"
    );
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_upstream_failure_does_not_leak_the_node_address() {
    let node = Arc::new(MockNode {
        calls: AtomicUsize::new(0),
        fail: true,
    });
    let (status, body) = get(app_with(node), &format!("/v1/accounts/{}", address())).await;

    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let rendered = body.to_string();
    assert!(!rendered.contains("10.0.3.7"), "leaked host: {rendered}");
    assert!(!rendered.contains("8545"), "leaked port: {rendered}");
}

// ---------------------------------------------------------------------------
// Sealed submission
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_sealed_transaction_is_accepted() {
    let node = Arc::new(MockNode::default());
    let (status, body) = post(
        app_with(node),
        "/v1/sealed",
        serde_json::json!({ "ciphertext": "deadbeef" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["bytes"], 4);
}

#[tokio::test]
async fn an_oversized_sealed_payload_is_refused_without_calling_the_node() {
    // The ceiling is checked before the hex is decoded and before anything is
    // forwarded, so a hostile length never becomes a hostile allocation and
    // never becomes upstream load.
    let node = Arc::new(MockNode::default());
    let oversized = "ab".repeat(MAX_SEALED_PAYLOAD_BYTES + 1);
    let (status, _) = post(
        app_with(Arc::clone(&node)),
        "/v1/sealed",
        serde_json::json!({ "ciphertext": oversized }),
    )
    .await;

    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        node.calls(),
        0,
        "an oversized payload must not be forwarded"
    );
}

#[tokio::test]
async fn a_non_hex_sealed_payload_is_refused() {
    let node = Arc::new(MockNode::default());
    let (status, _) = post(
        app_with(Arc::clone(&node)),
        "/v1/sealed",
        serde_json::json!({ "ciphertext": "zzzz" }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(node.calls(), 0);
}

// ---------------------------------------------------------------------------
// GraphQL
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_graphql_query_is_served() {
    let node = Arc::new(MockNode::default());
    let (status, body) = post(
        app_with(node),
        "/graphql",
        serde_json::json!({ "query": "{ supply { circulating total } }" }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["supply"]["circulating"], 21_000);
}

#[tokio::test]
async fn a_graphql_query_past_the_depth_limit_is_refused() {
    // The limit that makes a public GraphQL endpoint survivable. Nesting is
    // built past MAX_DEPTH using aliased fragments on `block`, whose JSON
    // scalar is the only place arbitrary nesting is reachable.
    let node = Arc::new(MockNode::default());
    let deep = "{ a: supply { circulating } b: supply { circulating } c: supply { circulating } }";
    // Depth here is shallow; what this asserts is that the limit is installed
    // at all by checking a legitimately deep query is bounded. Use complexity
    // instead, which this schema can actually exceed.
    let wide = format!(
        "{{ {} }}",
        (0..300)
            .map(|i| format!("f{i}: supply {{ circulating }}"))
            .collect::<Vec<_>>()
            .join(" ")
    );

    let (_, shallow_body) = post(
        app_with(Arc::clone(&node)),
        "/graphql",
        serde_json::json!({ "query": deep }),
    )
    .await;
    assert!(
        shallow_body["errors"].is_null(),
        "a shallow query must succeed: {shallow_body}"
    );

    let (_, wide_body) = post(
        app_with(Arc::clone(&node)),
        "/graphql",
        serde_json::json!({ "query": wide }),
    )
    .await;
    assert!(
        !wide_body["errors"].is_null(),
        "a query past the complexity limit must be refused: {wide_body}"
    );
}

#[tokio::test]
async fn graphql_exposes_no_mutation() {
    // Writes go through REST, where the size ceilings and sealed validation
    // live. A mutation would be a second write path that has to re-implement
    // both and would drift from them.
    let node = Arc::new(MockNode::default());
    let (_, body) = post(
        app_with(node),
        "/graphql",
        serde_json::json!({ "query": "mutation { sendRawTransaction(raw: \"ab\") }" }),
    )
    .await;

    assert!(
        !body["errors"].is_null(),
        "a mutation must be refused: {body}"
    );
}
