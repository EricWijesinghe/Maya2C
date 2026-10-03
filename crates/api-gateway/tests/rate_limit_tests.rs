//! Per-client rate limiting: the public gateway's only defence against one
//! client flooding the node, so its keying is tested as carefully as its limit.
//!
//! Behind a proxy every request arrives from the proxy's own address, so the
//! client is identified by a header the proxy sets. Which header, and which
//! entry of it, decides whether a client can spoof its way around the limit.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use maya_api_gateway::error::GatewayError;
use maya_api_gateway::limit::{ClientIp, RateLimit};
use maya_api_gateway::node::{Balance, ChainInfo, FeeInfo, NodeClient, Supply};
use tower::ServiceExt;

struct QuietNode;

#[async_trait]
impl NodeClient for QuietNode {
    async fn get_balance(&self, _address: &str) -> Result<Balance, GatewayError> {
        Ok(Balance {
            balance: 0,
            nonce: 0,
        })
    }
    async fn get_block_by_height(&self, _height: u64) -> Result<serde_json::Value, GatewayError> {
        Ok(serde_json::Value::Null)
    }
    async fn get_supply(&self) -> Result<Supply, GatewayError> {
        Ok(Supply {
            circulating: 0,
            total: 0,
        })
    }
    async fn send_raw_transaction(&self, _raw: &str) -> Result<String, GatewayError> {
        Ok(String::new())
    }
    async fn get_fee_info(&self) -> Result<FeeInfo, GatewayError> {
        Ok(FeeInfo {
            active: false,
            base_fee: 0,
            collector: String::new(),
        })
    }
    async fn get_chain_info(&self) -> Result<ChainInfo, GatewayError> {
        Ok(ChainInfo {
            genesis: "00".repeat(32),
            chain_id: None,
        })
    }
}

/// Two requests allowed at once, refilled slowly enough that a test never
/// sees a refill.
fn limits(client_ip: ClientIp) -> RateLimit {
    RateLimit {
        per_second: 1,
        burst: 2,
        client_ip,
    }
}

fn app(client_ip: ClientIp) -> axum::Router {
    maya_api_gateway::app_limited(Arc::new(QuietNode), &limits(client_ip)).expect("valid limits")
}

async fn status(app: &axum::Router, peer: &str, headers: &[(&str, &str)]) -> StatusCode {
    let mut request = Request::builder().uri("/health");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut request = request.body(Body::empty()).expect("request");
    let peer: SocketAddr = peer.parse().expect("peer");
    request.extensions_mut().insert(ConnectInfo(peer));
    app.clone()
        .oneshot(request)
        .await
        .expect("response")
        .status()
}

#[tokio::test]
async fn a_client_over_its_burst_is_refused_with_429() {
    let app = app(ClientIp::Socket);
    assert_eq!(status(&app, "203.0.113.1:5000", &[]).await, StatusCode::OK);
    assert_eq!(status(&app, "203.0.113.1:5001", &[]).await, StatusCode::OK);
    assert_eq!(
        status(&app, "203.0.113.1:5002", &[]).await,
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[tokio::test]
async fn one_client_exhausting_its_budget_does_not_block_another() {
    let app = app(ClientIp::Socket);
    for _ in 0..3 {
        status(&app, "203.0.113.1:5000", &[]).await;
    }
    assert_eq!(status(&app, "198.51.100.9:5000", &[]).await, StatusCode::OK);
}

#[tokio::test]
async fn in_socket_mode_a_forged_header_is_ignored() {
    // Directly exposed, no proxy: a header is whatever the client wrote.
    let app = app(ClientIp::Socket);
    for n in 0..2 {
        let forged = format!("10.0.0.{n}");
        status(&app, "203.0.113.1:5000", &[("cf-connecting-ip", &forged)]).await;
    }
    let refused = status(
        &app,
        "203.0.113.1:5000",
        &[("cf-connecting-ip", "10.9.9.9")],
    )
    .await;
    assert_eq!(refused, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn behind_cloudflare_each_visitor_is_keyed_by_cf_connecting_ip() {
    // Every request comes from the tunnel on loopback; Cloudflare's edge
    // overwrites CF-Connecting-IP with the visitor's real address.
    let app = app(ClientIp::CfConnectingIp);
    let visitor = [("cf-connecting-ip", "203.0.113.7")];
    assert_eq!(
        status(&app, "127.0.0.1:40000", &visitor).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, "127.0.0.1:40001", &visitor).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&app, "127.0.0.1:40002", &visitor).await,
        StatusCode::TOO_MANY_REQUESTS
    );

    let other = [("cf-connecting-ip", "198.51.100.3")];
    assert_eq!(
        status(&app, "127.0.0.1:40003", &other).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn x_forwarded_for_uses_the_last_hop_which_a_client_cannot_forge() {
    // A client may prepend anything; the proxy in front appends the address
    // it actually saw. Keying on the first entry would let a client rotate it.
    let app = app(ClientIp::XForwardedForLast);
    for n in 0..2 {
        let chain = format!("10.0.0.{n}, 203.0.113.7");
        status(&app, "127.0.0.1:40000", &[("x-forwarded-for", &chain)]).await;
    }
    let refused = status(
        &app,
        "127.0.0.1:40000",
        &[("x-forwarded-for", "10.9.9.9, 203.0.113.7")],
    )
    .await;
    assert_eq!(refused, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn a_request_without_the_header_is_keyed_by_its_socket() {
    // A local health check that did not come through the proxy still gets a
    // key of its own rather than an error.
    let app = app(ClientIp::CfConnectingIp);
    assert_eq!(status(&app, "127.0.0.1:40000", &[]).await, StatusCode::OK);
}

#[test]
fn zero_limits_are_refused_at_construction() {
    let result = maya_api_gateway::app_limited(
        Arc::new(QuietNode),
        &RateLimit {
            per_second: 0,
            burst: 2,
            client_ip: ClientIp::Socket,
        },
    );
    assert!(result.is_err());
}
