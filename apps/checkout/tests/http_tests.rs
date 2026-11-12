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
use maya_checkout::http::{AppState, ReadLimiter, router};
use maya_checkout::invoice::{Delivery, UnmatchedPayment, UnmatchedReason};
use maya_checkout::store::Store;
use maya_checkout::{CheckoutError, Result};
use primitive_types::U256;
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
        Ok(Tip {
            height: 100,
            chain_id: Some("maya-testnet-1".into()),
        })
    }
    async fn blocks(&self, _from: u64, _count: u64) -> Result<Vec<BlockOutputs>> {
        Ok(vec![])
    }
}

fn app(name: &str, fail_chain: bool) -> Router {
    let path = std::env::temp_dir().join(format!(
        "maya-checkout-http-{}-{name}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let store = Store::open(&path).unwrap();
    with_chain(
        Arc::new(Mutex::new(store)),
        Arc::new(FakeChain { fail: fail_chain }),
    )
}

fn with_chain(store: Arc<Mutex<Store>>, chain: Arc<dyn ChainSource>) -> Router {
    router(Arc::new(AppState {
        store,
        chain,
        network: Network::Testnet,
        pay_to: [9; 32],
        api_token: Zeroizing::new(TOKEN.as_bytes().to_vec()),
        reads: ReadLimiter::default(),
    }))
}

fn fresh_store(name: &str) -> Arc<Mutex<Store>> {
    let path = std::env::temp_dir().join(format!(
        "maya-checkout-http-{}-{name}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    Arc::new(Mutex::new(Store::open(&path).unwrap()))
}

/// A node that answers the tip only once the test releases it.
struct GatedChain {
    release: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl ChainSource for GatedChain {
    async fn tip(&self) -> Result<Tip> {
        self.release.notified().await;
        Ok(Tip {
            height: 100,
            chain_id: Some("maya-testnet-1".into()),
        })
    }
    async fn blocks(&self, _from: u64, _count: u64) -> Result<Vec<BlockOutputs>> {
        Ok(vec![])
    }
}

/// Gives a request the peer address that `serve` with connect info would add.
fn with_peer(req: axum::http::request::Builder) -> axum::http::request::Builder {
    req.extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
        [203, 0, 113, 7],
        40000,
    ))))
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Vec<u8>) {
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
    let response = app
        .clone()
        .oneshot(with_peer(req).body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

fn invoice_body(amount: &str, webhook: &str) -> Value {
    json!({ "amount": amount, "memo": "order-1", "expires_in_blocks": 50, "webhook_url": webhook })
}

#[tokio::test]
async fn creating_an_invoice_needs_the_bearer_token() {
    let app = app("auth", false);
    let body = invoice_body("500", PUBLIC_HOOK);
    assert_eq!(
        call(&app, "POST", "/v1/invoices", None, Some(body.clone()))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/invoices",
            Some("not-the-token"),
            Some(body)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_valid_request_returns_a_payment_link_and_pending_status() {
    let app = app("create", false);
    let (status, bytes) = call(
        &app,
        "POST",
        "/v1/invoices",
        Some(TOKEN),
        Some(invoice_body("500", PUBLIC_HOOK)),
    )
    .await;
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
        let (status, _) = call(
            &app,
            "POST",
            "/v1/invoices",
            Some(TOKEN),
            Some(invoice_body(amount, PUBLIC_HOOK)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "accepted amount {amount}");
    }
    let mut long = invoice_body("500", PUBLIC_HOOK);
    long["memo"] = json!("m".repeat(200));
    assert_eq!(
        call(&app, "POST", "/v1/invoices", Some(TOKEN), Some(long))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
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
        let (status, _) = call(
            &app,
            "POST",
            "/v1/invoices",
            Some(TOKEN),
            Some(invoice_body("500", url)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "accepted webhook {url}");
    }
}

#[tokio::test]
async fn a_second_open_invoice_for_the_same_amount_is_quoted_a_unit_higher() {
    // N4: no 409 that a stranger's open invoice could trigger. The second invoice
    // quotes the next free base unit, and the response says both amounts.
    let app = app("clash", false);
    let body = invoice_body("777", PUBLIC_HOOK);
    assert_eq!(
        call(
            &app,
            "POST",
            "/v1/invoices",
            Some(TOKEN),
            Some(body.clone())
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (status, bytes) = call(&app, "POST", "/v1/invoices", Some(TOKEN), Some(body)).await;
    assert_eq!(status, StatusCode::CREATED);
    let view: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(view["amount"], "778");
    assert_eq!(view["requested_amount"], "777");
}

#[tokio::test]
async fn alerts_list_invoices_whose_delivered_payment_was_reorged_out() {
    let store = fresh_store("alerts");
    let app = with_chain(Arc::clone(&store), Arc::new(FakeChain { fail: false }));
    let (_, created) = call(
        &app,
        "POST",
        "/v1/invoices",
        Some(TOKEN),
        Some(invoice_body("500", PUBLIC_HOOK)),
    )
    .await;
    let id = serde_json::from_slice::<Value>(&created).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    store.lock().await.ledger_mut().invoices[0].reorged = true;

    let (status, _) = call(&app, "GET", "/v1/alerts", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = call(&app, "GET", "/v1/alerts", Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    let rows: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(rows["items"][0]["invoice_id"], id.as_str());
}

#[tokio::test]
async fn unmatched_payments_are_paged_and_the_page_size_is_capped() {
    let store = fresh_store("paging");
    {
        let mut guard = store.lock().await;
        for n in 0..3u64 {
            guard.ledger_mut().unmatched.push(UnmatchedPayment {
                txid: format!("t{n}"),
                index: 0,
                height: n,
                block_id: format!("b{n}"),
                recipient: [9; 32],
                amount: U256::from(n + 1),
                reason: UnmatchedReason::NoExactInvoice,
            });
        }
    }
    let app = with_chain(store, Arc::new(FakeChain { fail: false }));

    let (_, body) = call(&app, "GET", "/v1/unmatched?limit=2", Some(TOKEN), None).await;
    let page: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert_eq!(page["total"], 3);
    assert_eq!(page["next_offset"], 2);

    let (_, body) = call(
        &app,
        "GET",
        "/v1/unmatched?offset=2&limit=2",
        Some(TOKEN),
        None,
    )
    .await;
    let page: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert!(page["next_offset"].is_null());

    let (_, body) = call(&app, "GET", "/v1/unmatched?limit=100000", Some(TOKEN), None).await;
    let page: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn an_unknown_invoice_is_not_found_and_a_known_one_is_public_without_secrets() {
    let app = app("view", false);
    let (_, bytes) = call(
        &app,
        "POST",
        "/v1/invoices",
        Some(TOKEN),
        Some(invoice_body("900", PUBLIC_HOOK)),
    )
    .await;
    let id = serde_json::from_slice::<Value>(&bytes).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_eq!(
        call(&app, "GET", "/v1/invoices/nope", None, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, view) = call(&app, "GET", &format!("/v1/invoices/{id}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(view).unwrap();
    assert!(!text.contains(TOKEN));
    assert!(
        !text.contains("93.184.216.34"),
        "the public view leaks the webhook destination: {text}"
    );
}

#[tokio::test]
async fn the_qr_code_is_an_svg() {
    let app = app("qr", false);
    let (_, bytes) = call(
        &app,
        "POST",
        "/v1/invoices",
        Some(TOKEN),
        Some(invoice_body("400", PUBLIC_HOOK)),
    )
    .await;
    let id = serde_json::from_slice::<Value>(&bytes).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let response = app
        .clone()
        .oneshot(
            with_peer(Request::builder().uri(format!("/v1/invoices/{id}/qr.svg")))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/svg+xml");
}

#[tokio::test]
async fn an_unreachable_node_stops_creation_rather_than_guessing_a_height() {
    let app = app("down", true);
    let (status, _) = call(
        &app,
        "POST",
        "/v1/invoices",
        Some(TOKEN),
        Some(invoice_body("500", PUBLIC_HOOK)),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn a_slow_node_does_not_hold_the_ledger_while_an_invoice_is_created() {
    let store = fresh_store("slow-node");
    let release = Arc::new(tokio::sync::Notify::new());
    let app = with_chain(
        Arc::clone(&store),
        Arc::new(GatedChain {
            release: Arc::clone(&release),
        }),
    );
    let creating = tokio::spawn(async move {
        call(
            &app,
            "POST",
            "/v1/invoices",
            Some(TOKEN),
            Some(invoice_body("500", PUBLIC_HOOK)),
        )
        .await
    });
    // Let the request reach the node call and park there.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(
        store.try_lock().is_ok(),
        "the ledger lock was held across the node call"
    );
    release.notify_one();
    let (status, _) = creating.await.unwrap();
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn a_failed_webhook_can_be_resent_but_a_delivered_one_cannot() {
    let store = fresh_store("resend");
    let app = with_chain(Arc::clone(&store), Arc::new(FakeChain { fail: false }));
    let (_, created) = call(
        &app,
        "POST",
        "/v1/invoices",
        Some(TOKEN),
        Some(invoice_body("500", PUBLIC_HOOK)),
    )
    .await;
    let id: Value = serde_json::from_slice(&created).unwrap();
    let uri = format!("/v1/invoices/{}/resend", id["id"].as_str().unwrap());
    {
        let mut guard = store.lock().await;
        let inv = &mut guard.ledger_mut().invoices[0];
        inv.status = maya_checkout::invoice::Status::Confirmed;
        inv.delivery = Delivery::Failed;
        inv.attempts = 8;
    }

    let (status, body) = call(&app, "POST", &uri, Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    let view: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(view["delivery"], "due");
    assert_eq!(store.lock().await.ledger().invoices[0].attempts, 0);

    store.lock().await.ledger_mut().invoices[0].delivery = Delivery::Delivered;
    let (status, _) = call(&app, "POST", &uri, Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = call(&app, "POST", &uri, None, None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "resend needs the bearer token"
    );
}

#[tokio::test]
async fn unmatched_payments_are_listed_for_the_merchant_only() {
    let store = fresh_store("unmatched");
    store
        .lock()
        .await
        .ledger_mut()
        .unmatched
        .push(UnmatchedPayment {
            txid: "ab".into(),
            index: 0,
            height: 7,
            block_id: "b7".into(),
            recipient: [9; 32],
            amount: U256::from(999u64),
            reason: UnmatchedReason::NoExactInvoice,
        });
    let app = with_chain(store, Arc::new(FakeChain { fail: false }));

    let (status, _) = call(&app, "GET", "/v1/unmatched", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = call(&app, "GET", "/v1/unmatched", Some(TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    let page: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(page["items"][0]["txid"], "ab");
    assert_eq!(page["items"][0]["amount"], "999");
    assert_eq!(page["items"][0]["reason"], "no_exact_invoice");
}

#[test]
fn public_reads_are_budgeted_per_client_address() {
    // N2: one client cannot spend the budget of every other client.
    let limiter = ReadLimiter::default();
    let now = 1_800_000_000;
    let a: std::net::IpAddr = "198.51.100.1".parse().unwrap();
    let b: std::net::IpAddr = "198.51.100.2".parse().unwrap();
    for _ in 0..50 {
        assert!(limiter.admit(a, now));
    }
    assert!(
        !limiter.admit(a, now),
        "client A's 51st read in one second must be refused"
    );
    assert!(limiter.admit(b, now), "client B has its own budget");
    assert!(
        limiter.admit(a, now + 1),
        "A's budget refills with the next second"
    );
}

#[test]
fn public_reads_have_a_global_ceiling_across_clients() {
    let limiter = ReadLimiter::default();
    let now = 1_800_000_000;
    let mut admitted = 0u32;
    for n in 0..(maya_checkout::http::GLOBAL_READS_PER_SEC + 100) {
        let [hi, lo] = u16::try_from(n).unwrap().to_be_bytes();
        let ip = std::net::IpAddr::from([10, 0, hi, lo]);
        if limiter.admit(ip, now) {
            admitted += 1;
        }
    }
    assert_eq!(admitted, maya_checkout::http::GLOBAL_READS_PER_SEC);
}
