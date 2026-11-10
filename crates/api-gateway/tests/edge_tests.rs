//! Malformed JSON-RPC bodies and attempts to dodge the per-client limiter.
//!
//! Neither may reach the node, and neither may crash the handler.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use maya_api_gateway::error::GatewayError;
use maya_api_gateway::limit::{ClientIp, RateLimit};
use maya_api_gateway::node::{Balance, ChainInfo, FeeInfo, NodeClient, Supply};
use tower::ServiceExt;

/// Counts every call; answers with fixed data.
#[derive(Default)]
struct CountingNode(AtomicUsize);

impl CountingNode {
    fn calls(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }
    fn hit(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[async_trait]
impl NodeClient for CountingNode {
    async fn get_balance(&self, _address: &str) -> Result<Balance, GatewayError> {
        self.hit();
        Ok(Balance {
            balance: "0".to_string(),
            nonce: 0,
        })
    }
    async fn get_block_by_height(&self, _height: u64) -> Result<serde_json::Value, GatewayError> {
        self.hit();
        Ok(serde_json::Value::Null)
    }
    async fn get_supply(&self) -> Result<Supply, GatewayError> {
        self.hit();
        Ok(Supply {
            circulating: "0".to_string(),
            total: "0".to_string(),
        })
    }
    async fn send_raw_transaction(&self, _raw: &str) -> Result<String, GatewayError> {
        self.hit();
        Ok(String::new())
    }
    async fn get_fee_info(&self) -> Result<FeeInfo, GatewayError> {
        self.hit();
        Ok(FeeInfo {
            active: false,
            base_fee: 0,
            collector: String::new(),
        })
    }
    async fn get_chain_info(&self) -> Result<ChainInfo, GatewayError> {
        self.hit();
        Ok(ChainInfo {
            genesis: "00".repeat(32),
            chain_id: None,
            height: 0,
        })
    }
}

async fn rpc(node: &Arc<CountingNode>, body: Vec<u8>) -> (StatusCode, serde_json::Value) {
    let response = maya_api_gateway::app(Arc::clone(node) as Arc<dyn NodeClient>)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/rpc")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = response.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

fn code(body: &serde_json::Value) -> i64 {
    body["error"]["code"].as_i64().expect("an error code")
}

#[tokio::test]
async fn bodies_that_are_not_json_get_parse_error_with_null_id() {
    let node = Arc::new(CountingNode::default());
    let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    let bodies: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"{".to_vec(),
        b"not json".to_vec(),
        br#"{"jsonrpc":"2.0","id":1,"method":"get_supply""#.to_vec(),
        vec![0xff, 0xfe, 0x00],
        deep.into_bytes(), // past serde_json's recursion limit: Err, not a stack overflow
    ];
    for body in bodies {
        let (status, reply) = rpc(&node, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body:?}");
        assert_eq!(code(&reply), -32_700, "{body:?}");
        assert_eq!(reply["id"], serde_json::Value::Null);
        assert_eq!(reply["jsonrpc"], "2.0");
    }
    assert_eq!(node.calls(), 0);
}

#[tokio::test]
async fn json_that_is_not_a_request_object_is_invalid_request() {
    let node = Arc::new(CountingNode::default());
    for body in ["42", "\"get_supply\"", "null", "true", "[]"] {
        let (_, reply) = rpc(&node, body.as_bytes().to_vec()).await;
        assert_eq!(code(&reply), -32_600, "{body}");
    }
    assert_eq!(node.calls(), 0);
}

#[tokio::test]
async fn an_invalid_request_echoes_its_id_and_a_bad_method_type_is_invalid() {
    let node = Arc::new(CountingNode::default());
    let (_, reply) = rpc(&node, br#"{"jsonrpc":"2.0","id":77,"method":5}"#.to_vec()).await;
    assert_eq!(code(&reply), -32_600);
    assert_eq!(reply["id"], 77);
    let (_, reply) = rpc(&node, br#"{"jsonrpc":"2.0","id":"a"}"#.to_vec()).await;
    assert_eq!(code(&reply), -32_600, "method missing");
    let (_, reply) = rpc(&node, br#"{"jsonrpc":2.0,"id":1,"method":"get_supply"}"#.to_vec()).await;
    assert_eq!(code(&reply), -32_600, "version must be the string 2.0");
    assert_eq!(node.calls(), 0);
}

#[tokio::test]
async fn unknown_methods_are_not_found_and_non_array_params_are_invalid() {
    let node = Arc::new(CountingNode::default());
    let (_, reply) = rpc(
        &node,
        br#"{"jsonrpc":"2.0","id":3,"method":"no_such_method","params":[]}"#.to_vec(),
    )
    .await;
    assert_eq!(code(&reply), -32_601);
    assert_eq!(reply["id"], 3);
    for params in ["\"x\"", "7", "null", "{}"] {
        let body = format!(r#"{{"jsonrpc":"2.0","id":4,"method":"get_supply","params":{params}}}"#);
        let (_, reply) = rpc(&node, body.into_bytes()).await;
        assert_eq!(code(&reply), -32_602, "params {params}");
    }
    assert_eq!(node.calls(), 0);
}

// ---- limiter bypass attempts ------------------------------------------

fn limited(client_ip: ClientIp) -> axum::Router {
    let limits = RateLimit {
        per_second: 1,
        burst: 2,
        client_ip,
    };
    maya_api_gateway::app_limited(Arc::new(CountingNode::default()), &limits).expect("limits")
}

async fn status(app: &axum::Router, peer: &str, headers: &[(&str, &str)]) -> StatusCode {
    let mut request = Request::builder().uri("/health");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut request = request.body(Body::empty()).expect("request");
    let peer: SocketAddr = peer.parse().expect("peer");
    request.extensions_mut().insert(ConnectInfo(peer));
    app.clone().oneshot(request).await.expect("response").status()
}

const PEER: &str = "127.0.0.1:40000";

/// Sends `n` requests with the headers `make(i)` yields and returns how many
/// were answered 429.
async fn refused_of(
    app: &axum::Router,
    n: usize,
    peer: &str,
    make: impl Fn(usize) -> Vec<(&'static str, String)>,
) -> usize {
    let mut refused = 0;
    for i in 0..n {
        let owned = make(i);
        let headers: Vec<(&str, &str)> = owned.iter().map(|(k, v)| (*k, v.as_str())).collect();
        if status(app, peer, &headers).await == StatusCode::TOO_MANY_REQUESTS {
            refused += 1;
        }
    }
    refused
}

#[tokio::test]
async fn an_unparseable_forwarded_hop_falls_back_to_the_socket_not_a_free_key() {
    let app = limited(ClientIp::XForwardedForLast);
    for junk in ["garbage", "1.2.3.4,", "1.2.3.4, not-an-ip", "", "999.1.1.1"] {
        // Each junk value must land in the same (socket) bucket: burst 2 then 429.
        let app = limited(ClientIp::XForwardedForLast);
        let refused = refused_of(&app, 5, PEER, |_| vec![("x-forwarded-for", junk.to_string())]).await;
        assert_eq!(refused, 3, "junk {junk:?}");
    }
    // A rotating junk value is still one socket.
    let refused = refused_of(&app, 5, PEER, |i| vec![("x-forwarded-for", format!("junk{i}"))]).await;
    assert_eq!(refused, 3);
}

#[tokio::test]
async fn other_proxy_headers_do_not_choose_the_key() {
    let xff = limited(ClientIp::XForwardedForLast);
    let rotated_cf = refused_of(&xff, 6, PEER, |i| {
        vec![
            ("x-forwarded-for", "203.0.113.7".to_string()),
            ("cf-connecting-ip", format!("10.0.0.{i}")),
        ]
    })
    .await;
    assert_eq!(rotated_cf, 4, "cf-connecting-ip is ignored in XFF mode");

    let cf = limited(ClientIp::CfConnectingIp);
    let rotated_xff = refused_of(&cf, 6, PEER, |i| {
        vec![
            ("cf-connecting-ip", "203.0.113.7".to_string()),
            ("x-forwarded-for", format!("10.0.0.{i}")),
        ]
    })
    .await;
    assert_eq!(rotated_xff, 4, "x-forwarded-for is ignored in Cloudflare mode");
}

#[tokio::test]
async fn real_clients_behind_the_proxy_keep_separate_budgets() {
    let app = limited(ClientIp::XForwardedForLast);
    let hop = |ip: &str| vec![("x-forwarded-for", format!("evil, {ip}"))];
    assert_eq!(refused_of(&app, 4, PEER, |_| hop("203.0.113.7")).await, 2);
    assert_eq!(refused_of(&app, 2, PEER, |_| hop("203.0.113.8")).await, 0);
    assert_eq!(refused_of(&app, 2, PEER, |_| hop("2001:db8::1")).await, 0, "IPv6 key");
    assert_eq!(refused_of(&app, 2, PEER, |_| hop("2001:db8::2")).await, 0, "other IPv6 key");
    assert_eq!(refused_of(&app, 1, PEER, |_| hop("2001:db8::1")).await, 1);
}

#[tokio::test]
async fn a_second_forwarded_for_line_cannot_hide_the_proxys_hop() {
    // Two header lines mean the comma-joined list (RFC 9110 5.3): the proxy's
    // own entry is on the last line. Reading the first line would key on
    // whatever the client wrote there, and rotating it dodges the limit.
    let app = limited(ClientIp::XForwardedForLast);
    let refused = refused_of(&app, 6, PEER, |i| {
        vec![
            ("x-forwarded-for", format!("10.0.0.{i}")),
            ("x-forwarded-for", "203.0.113.7".to_string()),
        ]
    })
    .await;
    assert_eq!(refused, 4, "all six are the proxy-seen client 203.0.113.7");
}
