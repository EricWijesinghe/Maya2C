//! The HTTP API: authentication, input refusal, the SSRF gate at creation,
//! the amount clash rule, and what a public view must not reveal.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use maya_address::Network;
use maya_checkout::chain::{BlockOutputs, ChainSource, Tip};
use maya_checkout::http::{AppState, router};
use maya_checkout::store::Store;
use maya_checkout::{CheckoutError, Result};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tower::ServiceExt;
use zeroize::Zeroizing;

const TOKEN: &str = "api-token-api-token-api-token-0001";
const PUBLIC_HOOK: &str = "https://93.184.216.34/hook";

struct FakeChain {
    fail: bool,
}

#[async_trait]
impl ChainSource for FakeChain {
    async fn tip(&self) -> Result<Tip> {
        if self.fail {
            return Err(CheckoutError::Chain("down".into()));
        }
        Ok(Tip { height: 100, chain_id: Some("maya-testnet-1".into()) })
    }
    async fn blocks(&self, _from: u64, _count: u64) -> Result<Vec<BlockOutputs>> {
        Ok(vec![])
    }
}

fn app(name: &str, fail_chain: bool) -> Router {
    let path = std::env::temp_dir().join(format!("maya-checkout-http-{}-{name}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let store = Store::open(&path).unwrap();
    router(Arc::new(AppState {
        store: Arc::new(Mutex::new(store)),
        chain: Arc::new(FakeChain { fail: fail_chain }),
        network: Network::Testnet,
        pay_to: [9; 32],
        api_token: Zeroizing::new(TOKEN.as_bytes().to_vec()),
    }))
}

async fn call(app: &Router, method: &str, uri: &str, auth: Option<&str>, body: Option<Value>) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(token) = auth {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = match body {
        Some(json) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(json.to_string())
        }
        None => Body::empty(),
    };
    let response = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes().to_vec();
    (status, bytes)
}

fn invoice_body(amount: &str, webhook: &str) -> Value {
    json!({ "amount": amount, "memo": "order-1", "expires_in_blocks": 50, "webhook_url": webhook })
}

#[tokio::test]
async fn creating_an_invoice_needs_the_bearer_token() {
    let app = app("auth", false);
    let body = invoice_body("500", PUBLIC_HOOK);
    assert_eq!(call(&app, "POST", "/v1/invoices", None, Some(body.clone())).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(
        call(&app, "POST", "/v1/invoices", Some("not-the-token"), Some(body)).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_valid_request_returns_a_payment_link_and_pending_status() {
    let app = app("create", false);
    let (status, bytes) = call(&app, "POST", "/v1/invoices", Some(TOKEN), Some(invoice_body("500", PUBLIC_HOOK))).await;
    assert_eq!(status, StatusCode::CREATED);
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["status"], "pending");
    assert_eq!(json["amount"], "500");
    let uri = json["pay_uri"].as_str().unwrap();
    assert!(uri.starts_with("maya:tmaya1"), "{uri}");
    assert!(uri.ends_with("?amount=500&id=order-1"), "{uri}");
    assert_eq!(json["created_height"], 100);
    assert_eq!(json["expires_at_height"], 150);
}

#[tokio::test]
async fn bad_amounts_and_memos_are_refused() {
    let app = app("bad", false);
    for amount in ["0500", "0", "-5", "12.5", "abc"] {
        let (status, _) = call(&app, "POST", "/v1/invoices", Some(TOKEN), Some(invoice_body(amount, PUBLIC_HOOK))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "accepted amount {amount}");
    }
    let mut long = invoice_body("500", PUBLIC_HOOK);
    long["memo"] = json!("m".repeat(200));
    assert_eq!(call(&app, "POST", "/v1/invoices", Some(TOKEN), Some(long)).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_webhook_to_a_private_or_plain_address_is_refused_at_creation() {
    let app = app("ssrf", false);
    for url in [
        "https://127.0.0.1/hook",
        "https://169.254.169.254/latest",
        "http://93.184.216.34/hook",
        "https://user:pw@93.184.216.34/hook",
    ] {
        let (status, _) = call(&app, "POST", "/v1/invoices", Some(TOKEN), Some(invoice_body("500", url))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "accepted webhook {url}");
    }
}

#[tokio::test]
async fn a_second_open_invoice_for_the_same_amount_is_a_conflict() {
    let app = app("clash", false);
    let body = invoice_body("777", PUBLIC_HOOK);
    assert_eq!(call(&app, "POST", "/v1/invoices", Some(TOKEN), Some(body.clone())).await.0, StatusCode::CREATED);
    assert_eq!(call(&app, "POST", "/v1/invoices", Some(TOKEN), Some(body)).await.0, StatusCode::CONFLICT);
}

#[tokio::test]
async fn an_unknown_invoice_is_not_found_and_a_known_one_is_public_without_secrets() {
    let app = app("view", false);
    let (_, bytes) = call(&app, "POST", "/v1/invoices", Some(TOKEN), Some(invoice_body("900", PUBLIC_HOOK))).await;
    let id = serde_json::from_slice::<Value>(&bytes).unwrap()["id"].as_str().unwrap().to_owned();

    assert_eq!(call(&app, "GET", "/v1/invoices/nope", None, None).await.0, StatusCode::NOT_FOUND);
    let (status, view) = call(&app, "GET", &format!("/v1/invoices/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(view).unwrap();
    assert!(!text.contains(TOKEN));
    assert!(!text.contains("93.184.216.34"), "the public view leaks the webhook destination: {text}");
}

#[tokio::test]
async fn the_qr_code_is_an_svg() {
    let app = app("qr", false);
    let (_, bytes) = call(&app, "POST", "/v1/invoices", Some(TOKEN), Some(invoice_body("400", PUBLIC_HOOK))).await;
    let id = serde_json::from_slice::<Value>(&bytes).unwrap()["id"].as_str().unwrap().to_owned();
    let response = app
        .clone()
        .oneshot(Request::builder().uri(format!("/v1/invoices/{id}/qr.svg")).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
}

#[tokio::test]
async fn an_unreachable_node_stops_creation_rather_than_guessing_a_height() {
    let app = app("down", true);
    let (status, _) = call(&app, "POST", "/v1/invoices", Some(TOKEN), Some(invoice_body("500", PUBLIC_HOOK))).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
