//! The faucet over HTTP: the status codes, and what they mean.
//!
//! These drive the real router through `tower::Service` rather than binding a
//! port. The peer address is injected as `ConnectInfo` exactly as
//! `into_make_service_with_connect_info` does at runtime, so the limiter sees
//! the same thing it would see in production — which is the part worth testing.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use maya_faucet::Faucet;
use maya_faucet::http::{FORWARDED_FOR, FaucetService, router};
use maya_faucet::testing::{FailingDispenser, RecordingDispenser};
use tower::ServiceExt;

const DISPENSE: u64 = 100;
const DAILY_CAP: u64 = 1_000;

fn service(dispenser: Arc<RecordingDispenser>) -> Arc<FaucetService> {
    let faucet = Faucet::new(
        "maya-genesis-rc1",
        DISPENSE,
        DAILY_CAP,
        std::time::SystemTime::now(),
    )
    .expect("a testnet chain");
    Arc::new(FaucetService::new(faucet, dispenser))
}

fn address(byte: u8) -> String {
    hex::encode([byte; 32])
}

/// A request carrying `peer` as its connection address.
fn grant_request(peer: &str, address: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/request")
        .header("content-type", "application/json")
        .body(Body::from(format!(r#"{{"address":"{address}"}}"#)))
        .expect("request");

    let peer: SocketAddr = format!("{peer}:40000").parse().expect("peer address");
    request
        .extensions_mut()
        .insert(axum::extract::ConnectInfo(peer));
    request
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("the faucet answers in JSON")
}

#[tokio::test]
async fn a_first_request_is_funded() {
    let dispenser = Arc::new(RecordingDispenser::new());
    let response = router(service(Arc::clone(&dispenser)))
        .oneshot(grant_request("203.0.113.1", &address(1)))
        .await
        .expect("served");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["amount"], DISPENSE);
    assert_eq!(body["remaining_today"], DAILY_CAP - DISPENSE);

    let sent = dispenser.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0], ([1u8; 32], DISPENSE));
}

#[tokio::test]
async fn a_second_request_from_one_ip_is_a_429_naming_the_ip() {
    // Not a 400 and not a 500: a rate limit is the service working. Alerting
    // on 5xx has to mean the faucet is broken, not that it is busy.
    let dispenser = Arc::new(RecordingDispenser::new());
    let app = router(service(Arc::clone(&dispenser)));

    app.clone()
        .oneshot(grant_request("203.0.113.1", &address(1)))
        .await
        .expect("first");

    let response = app
        .oneshot(grant_request("203.0.113.1", &address(2)))
        .await
        .expect("second");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = body_json(response).await;
    assert_eq!(body["error"], "rate_limited_ip");
    assert!(
        body["retry_after"].as_u64().expect("a wait is the remedy") > 0,
        "a 429 must say how long to wait"
    );

    assert_eq!(
        dispenser.sent().len(),
        1,
        "the refused request sent nothing"
    );
}

#[tokio::test]
async fn one_address_from_two_ips_is_a_429_naming_the_address() {
    // The two refusals need different remedies. Telling someone behind a
    // shared connection that "this address already has funds" would be false.
    let dispenser = Arc::new(RecordingDispenser::new());
    let app = router(service(Arc::clone(&dispenser)));

    app.clone()
        .oneshot(grant_request("203.0.113.1", &address(7)))
        .await
        .expect("first");

    let response = app
        .oneshot(grant_request("198.51.100.2", &address(7)))
        .await
        .expect("second");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body_json(response).await["error"], "rate_limited_address");
}

#[tokio::test]
async fn a_malformed_address_is_a_400_and_consumes_nothing() {
    let dispenser = Arc::new(RecordingDispenser::new());
    let app = router(service(Arc::clone(&dispenser)));

    let response = app
        .clone()
        .oneshot(grant_request("203.0.113.1", "not-an-address"))
        .await
        .expect("served");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"], "bad_address");

    // The typo must not have spent the caller's day.
    let response = app
        .oneshot(grant_request("203.0.113.1", &address(1)))
        .await
        .expect("served");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_exhausted_budget_is_a_503() {
    // Distinct from a 429: nothing the caller does makes this clear sooner,
    // and "try a different address" would be the wrong advice.
    let dispenser = Arc::new(RecordingDispenser::new());
    let app = router(service(Arc::clone(&dispenser)));

    for byte in 0..10u8 {
        let response = app
            .clone()
            .oneshot(grant_request(&format!("203.0.113.{byte}"), &address(byte)))
            .await
            .expect("served");
        assert_eq!(response.status(), StatusCode::OK, "grant {byte}");
    }

    let response = app
        .oneshot(grant_request("198.51.100.1", &address(200)))
        .await
        .expect("served");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body_json(response).await["error"], "budget_exhausted");
    assert_eq!(dispenser.total(), DAILY_CAP);
}

#[tokio::test]
async fn a_node_failure_is_a_500_that_leaks_nothing() {
    // The grant is recorded and not given back. A submission failure the
    // caller could retry freely would be a way to spend the day's budget
    // without ever completing a grant.
    let faucet = Faucet::new(
        "maya-genesis-rc1",
        DISPENSE,
        DAILY_CAP,
        std::time::SystemTime::now(),
    )
    .expect("testnet");
    let app = router(Arc::new(FaucetService::new(
        faucet,
        Arc::new(FailingDispenser),
    )));

    let response = app
        .clone()
        .oneshot(grant_request("203.0.113.1", &address(1)))
        .await
        .expect("served");
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = body_json(response).await;
    let message = body["message"].as_str().expect("a message");
    assert!(
        !message.contains("node is down"),
        "an internal error must not reach the caller verbatim: {message}"
    );

    // And the window was consumed.
    let response = app
        .oneshot(grant_request("203.0.113.1", &address(2)))
        .await
        .expect("served");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn the_proxy_header_is_ignored_unless_the_proxy_is_trusted() {
    // The important negative. If the header were read by default, a caller
    // could name a fresh bucket per request and the limiter would be
    // decorative.
    let dispenser = Arc::new(RecordingDispenser::new());
    let app = router(service(Arc::clone(&dispenser)));

    app.clone()
        .oneshot(grant_request("203.0.113.1", &address(1)))
        .await
        .expect("first");

    let mut request = grant_request("203.0.113.1", &address(2));
    request
        .headers_mut()
        .insert(FORWARDED_FOR, "8.8.8.8".parse().expect("header"));

    let response = app.oneshot(request).await.expect("served");
    assert_eq!(
        response.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "a caller must not be able to choose their own rate-limit bucket"
    );
}

#[tokio::test]
async fn a_trusted_proxy_supplies_the_client_address() {
    // With a proxy in front, every request arrives from the proxy; keying on
    // the peer address would give the whole internet one shared bucket.
    let faucet = Faucet::new(
        "maya-genesis-rc1",
        DISPENSE,
        DAILY_CAP,
        std::time::SystemTime::now(),
    )
    .expect("testnet");
    let app = router(Arc::new(
        FaucetService::new(faucet, Arc::new(RecordingDispenser::new())).trust_proxy(true),
    ));

    // Two clients, one proxy. Both must be funded.
    for (client, byte) in [("198.51.100.7", 1u8), ("198.51.100.8", 2u8)] {
        let mut request = grant_request("10.0.0.1", &address(byte));
        request
            .headers_mut()
            .insert(FORWARDED_FOR, client.parse().expect("header"));

        let response = app.clone().oneshot(request).await.expect("served");
        assert_eq!(response.status(), StatusCode::OK, "client {client}");
    }
}

#[tokio::test]
async fn a_trusted_proxy_takes_the_last_hop_not_the_first() {
    // A caller sending `X-Forwarded-For: 1.1.1.1` gets that prepended to what
    // the proxy appends. Reading the first entry would hand the bucket choice
    // straight back to the caller.
    let faucet = Faucet::new(
        "maya-genesis-rc1",
        DISPENSE,
        DAILY_CAP,
        std::time::SystemTime::now(),
    )
    .expect("testnet");
    let app = router(Arc::new(
        FaucetService::new(faucet, Arc::new(RecordingDispenser::new())).trust_proxy(true),
    ));

    let mut first = grant_request("10.0.0.1", &address(1));
    first
        .headers_mut()
        .insert(FORWARDED_FOR, "198.51.100.7".parse().expect("header"));
    let response = app.clone().oneshot(first).await.expect("served");
    assert_eq!(response.status(), StatusCode::OK);

    // Same real client, now claiming a different origin.
    let mut second = grant_request("10.0.0.1", &address(2));
    second.headers_mut().insert(
        FORWARDED_FOR,
        "1.1.1.1, 198.51.100.7".parse().expect("header"),
    );
    let response = app.oneshot(second).await.expect("served");
    assert_eq!(
        response.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the spoofed first entry must not create a new bucket"
    );
}

#[tokio::test]
async fn status_reports_the_budget_without_revealing_the_key() {
    let dispenser = Arc::new(RecordingDispenser::new());
    let app = router(service(Arc::clone(&dispenser)));

    app.clone()
        .oneshot(grant_request("203.0.113.1", &address(1)))
        .await
        .expect("grant");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/status")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("served");

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["chain_id"], "maya-genesis-rc1");
    assert_eq!(body["spent_today"], DISPENSE);
    assert_eq!(body["daily_cap"], DAILY_CAP);

    let rendered = body.to_string();
    for forbidden in ["key", "secret", "seed"] {
        assert!(
            !rendered.contains(forbidden),
            "a public status page must not mention {forbidden}: {rendered}"
        );
    }
}
