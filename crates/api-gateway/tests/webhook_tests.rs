//! Webhook subscriptions: the gateway making outbound requests to URLs a
//! stranger chose. Every test here is about one of the ways that goes wrong:
//! a target inside the operator's network (SSRF), a forged delivery (HMAC), a
//! caller filling the table (caps), or a receiver that is down (retries and
//! the dead-letter state).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use maya_api_gateway::events::{Event, Events, EventsConfig};
use maya_api_gateway::ssrf;
use maya_api_gateway::webhooks::{self, Transport, WebhookSecret};
use reqwest::Url;
use tower::ServiceExt;

const ALICE: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const BOB: &str = "bb00000000000000000000000000000000000000000000000000000000000002";

// --- SSRF ------------------------------------------------------------------

#[test]
fn private_loopback_and_link_local_literals_are_refused() {
    for url in [
        "https://127.0.0.1/hook",
        "https://127.8.9.10/hook",
        "https://10.0.0.5/hook",
        "https://172.16.0.1/hook",
        "https://192.168.1.1/hook",
        "https://169.254.169.254/latest/meta-data",
        "https://100.64.0.1/hook",
        "https://0.0.0.0/hook",
        "https://[::1]/hook",
        "https://[fe80::1]/hook",
        "https://[fc00::1]/hook",
        "https://[fd12:3456::1]/hook",
        "https://[::ffff:127.0.0.1]/hook",
        "https://[::ffff:10.0.0.1]/hook",
        "https://[64:ff9b::a9fe:a9fe]/hook",
        "https://[::]/hook",
        "https://224.0.0.1/hook",
        "https://255.255.255.255/hook",
    ] {
        assert!(
            ssrf::check_url(url, false).is_err(),
            "{url} must be refused"
        );
    }
}

#[test]
fn a_public_https_target_is_accepted() {
    let url = ssrf::check_url("https://93.184.216.34:8443/hook?x=1", false).expect("public");
    assert_eq!(url.host_str(), Some("93.184.216.34"));
    assert!(ssrf::check_url("https://hooks.example.com/maya", false).is_ok());
}

#[test]
fn plain_http_is_refused_unless_the_dev_flag_is_set() {
    assert!(ssrf::check_url("http://93.184.216.34/hook", false).is_err());
    assert!(ssrf::check_url("http://93.184.216.34/hook", true).is_ok());
    // The dev flag relaxes the scheme only, never the address rules.
    assert!(ssrf::check_url("http://127.0.0.1/hook", true).is_err());
}

#[test]
fn other_schemes_credentials_and_overlong_urls_are_refused() {
    assert!(ssrf::check_url("file:///etc/passwd", true).is_err());
    assert!(ssrf::check_url("gopher://93.184.216.34/", true).is_err());
    assert!(ssrf::check_url("https://user:pw@hooks.example.com/", false).is_err());
    assert!(ssrf::check_url("not a url", false).is_err());
    let long = format!(
        "https://hooks.example.com/{}",
        "a".repeat(ssrf::MAX_URL_LEN)
    );
    assert!(ssrf::check_url(&long, false).is_err());
}

#[test]
fn every_public_ip_check_agrees_with_the_obvious_cases() {
    assert!(ssrf::is_public_ip("8.8.8.8".parse().unwrap()));
    assert!(ssrf::is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    assert!(!ssrf::is_public_ip("192.168.0.1".parse().unwrap()));
    assert!(!ssrf::is_public_ip("::1".parse().unwrap()));
}

#[tokio::test]
async fn a_name_that_resolves_to_loopback_is_refused_after_dns() {
    // `localhost` passes the static check (it is a name, not an address) and
    // is caught only once resolved: the re-check after DNS is the point.
    let url = ssrf::check_url("https://localhost:9/hook", false).expect("static check");
    let error = ssrf::resolve_public(&url)
        .await
        .expect_err("resolves to loopback");
    assert!(error.to_string().contains("refused"), "{error}");
}

#[tokio::test]
async fn the_http_transport_re_checks_at_delivery_time() {
    // A target that passed registration is resolved again before every
    // delivery, so DNS that later points at loopback is refused, not followed.
    let transport = webhooks::HttpTransport::new(false);
    let url = Url::parse("https://localhost:9/hook").unwrap();
    let error = transport
        .post(&url, b"{}".to_vec(), "sha256=00".to_string())
        .await
        .expect_err("refused");
    assert!(error.contains("refused"), "{error}");
}

// --- HMAC ------------------------------------------------------------------

#[test]
fn hmac_matches_the_rfc_4231_vector() {
    // RFC 4231 test case 2.
    assert_eq!(
        webhooks::mac_hex(b"Jefe", b"what do ya want for nothing?"),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
}

#[test]
fn a_signed_body_verifies_and_a_tampered_one_does_not() {
    let secret = WebhookSecret::generate().expect("entropy");
    let secret_hex = secret.to_hex();
    let body = br#"{"event":{"height":7}}"#;
    let header = webhooks::sign(&secret, body);
    assert!(header.starts_with("sha256="));
    assert!(webhooks::verify(&secret_hex, body, &header));
    assert!(!webhooks::verify(
        &secret_hex,
        br#"{"event":{"height":8}}"#,
        &header
    ));
    let other = WebhookSecret::generate().expect("entropy").to_hex();
    assert!(!webhooks::verify(&other, body, &header));
    assert!(!webhooks::verify(&secret_hex, body, "sha256=zz"));
    assert!(!webhooks::verify(
        &secret_hex,
        body,
        &header.replace("sha256=", "")
    ));
    assert!(!webhooks::verify("not hex", body, &header));
}

#[test]
fn the_secret_never_prints() {
    let secret = WebhookSecret::generate().expect("entropy");
    let shown = format!("{secret:?}");
    assert!(!shown.contains(&*secret.to_hex()), "{shown}");
    assert!(shown.contains("redacted"));
}

// --- registration ----------------------------------------------------------

/// Records every delivery; fails the first `fail_first` attempts.
#[derive(Default)]
struct Recorder {
    fail_first: Mutex<usize>,
    attempts: Mutex<usize>,
    delivered: Mutex<Vec<(Vec<u8>, String)>>,
}

#[async_trait]
impl Transport for Recorder {
    async fn post(&self, _url: &Url, body: Vec<u8>, signature: String) -> Result<(), String> {
        *self.attempts.lock().unwrap() += 1;
        let mut fail = self.fail_first.lock().unwrap();
        if *fail > 0 {
            *fail -= 1;
            return Err("receiver answered 503".to_string());
        }
        self.delivered.lock().unwrap().push((body, signature));
        Ok(())
    }
}

fn config() -> EventsConfig {
    EventsConfig {
        max_webhooks_per_caller: 2,
        max_attempts: 3,
        backoff_base: Duration::from_millis(5),
        backoff_max: Duration::from_millis(20),
        client_ip: maya_api_gateway::limit::ClientIp::Socket,
        ..EventsConfig::default()
    }
}

fn setup(fail_first: usize) -> (Arc<Events>, Arc<Recorder>, axum::Router) {
    let recorder = Arc::new(Recorder {
        fail_first: Mutex::new(fail_first),
        ..Recorder::default()
    });
    let events = Events::new(config(), Arc::clone(&recorder) as Arc<dyn Transport>);
    let router = maya_api_gateway::events::router(Arc::clone(&events));
    (events, recorder, router)
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    peer: &str,
    auth: Option<&str>,
    body: String,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(secret) = auth {
        request = request.header("authorization", format!("Bearer {secret}"));
    }
    let mut request = request.body(Body::from(body)).unwrap();
    let peer: SocketAddr = peer.parse().unwrap();
    request.extensions_mut().insert(ConnectInfo(peer));
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

async fn register(
    app: &axum::Router,
    peer: &str,
    address: &str,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({ "url": "https://hooks.example.com/maya", "address": address });
    call(app, "POST", "/v1/webhooks", peer, None, body.to_string()).await
}

#[tokio::test]
async fn registering_returns_an_id_and_a_secret_once() {
    let (_events, _recorder, app) = setup(0);
    let (status, created) = register(&app, "203.0.113.1:1", ALICE).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_str().unwrap();
    let secret = created["secret"].as_str().unwrap();
    assert_eq!(secret.len(), 64);

    let (status, shown) = call(
        &app,
        "GET",
        &format!("/v1/webhooks/{id}"),
        "203.0.113.1:1",
        Some(secret),
        String::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(shown["state"], "active");
    assert!(
        !shown.to_string().contains(secret),
        "secret is returned once"
    );
}

#[tokio::test]
async fn a_private_target_is_refused_at_registration() {
    let (_events, _recorder, app) = setup(0);
    let body = serde_json::json!({ "url": "https://169.254.169.254/latest" });
    let (status, error) = call(
        &app,
        "POST",
        "/v1/webhooks",
        "203.0.113.1:1",
        None,
        body.to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
}

#[tokio::test]
async fn a_name_resolving_to_loopback_is_refused_at_registration() {
    let events = Events::new(config(), Arc::new(webhooks::HttpTransport::new(false)));
    let app = maya_api_gateway::events::router(events);
    let body = serde_json::json!({ "url": "https://localhost/hook" });
    let (status, error) = call(
        &app,
        "POST",
        "/v1/webhooks",
        "203.0.113.1:1",
        None,
        body.to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert!(
        error["error"].as_str().unwrap().contains("refused"),
        "{error}"
    );
}

#[tokio::test]
async fn a_bad_filter_is_refused() {
    let (_events, _recorder, app) = setup(0);
    let (status, _) = register(&app, "203.0.113.1:1", "nothex").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body = serde_json::json!({ "url": "https://hooks.example.com/", "asset": "x".repeat(200) });
    let (status, _) = call(
        &app,
        "POST",
        "/v1/webhooks",
        "203.0.113.1:1",
        None,
        body.to_string(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_oversized_registration_body_is_refused() {
    let (_events, _recorder, app) = setup(0);
    let body = format!(
        r#"{{"url":"https://hooks.example.com/{}"}}"#,
        "a".repeat(64 * 1024)
    );
    let (status, _) = call(&app, "POST", "/v1/webhooks", "203.0.113.1:1", None, body).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn a_caller_is_capped_and_another_is_not() {
    let (_events, _recorder, app) = setup(0);
    for _ in 0..2 {
        assert_eq!(
            register(&app, "203.0.113.1:1", ALICE).await.0,
            StatusCode::CREATED
        );
    }
    assert_eq!(
        register(&app, "203.0.113.1:2", ALICE).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        register(&app, "198.51.100.9:1", ALICE).await.0,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn ipv6_callers_in_one_slash_64_share_a_cap() {
    let (_events, _recorder, app) = setup(0);
    assert_eq!(
        register(&app, "[2001:db8:1:2::1]:1", ALICE).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        register(&app, "[2001:db8:1:2::ffff]:1", ALICE).await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        register(&app, "[2001:db8:1:2:abcd::9]:1", ALICE).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        register(&app, "[2001:db8:1:3::1]:1", ALICE).await.0,
        StatusCode::CREATED
    );
}

#[test]
fn teredo_site_local_and_discard_ranges_are_refused() {
    for ip in ["2001::1", "fec0::1", "64:ff9b:1::1", "100::1"] {
        assert!(!ssrf::is_public_ip(ip.parse().unwrap()), "{ip}");
    }
}

#[tokio::test]
async fn the_wrong_secret_sees_nothing_and_deletes_nothing() {
    let (_events, _recorder, app) = setup(0);
    let (_, created) = register(&app, "203.0.113.1:1", ALICE).await;
    let id = created["id"].as_str().unwrap();
    let uri = format!("/v1/webhooks/{id}");
    let wrong = "00".repeat(32);
    assert_eq!(
        call(
            &app,
            "GET",
            &uri,
            "203.0.113.1:1",
            Some(&wrong),
            String::new()
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", &uri, "203.0.113.1:1", None, String::new())
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &uri,
            "203.0.113.1:1",
            Some(&wrong),
            String::new()
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let secret = created["secret"].as_str().unwrap();
    assert_eq!(
        call(
            &app,
            "DELETE",
            &uri,
            "203.0.113.1:1",
            Some(secret),
            String::new()
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &uri,
            "203.0.113.1:1",
            Some(secret),
            String::new()
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // Deleting frees the caller's slot.
    for _ in 0..2 {
        assert_eq!(
            register(&app, "203.0.113.1:1", ALICE).await.0,
            StatusCode::CREATED
        );
    }
}

// --- delivery --------------------------------------------------------------

fn transfer(height: u64, sender: &str, recipient: &str) -> serde_json::Value {
    serde_json::json!({
        "height": height,
        "header": { "timestamp": 1 },
        "transactions": [{
            "txid": format!("{height:064x}"),
            "sender": sender,
            "outputs": [{ "recipient": recipient, "amount": "5" }],
        }],
        "raw": "00",
    })
}

async fn wait_for(mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not reached");
}

fn publish(events: &Events, height: u64, block: &serde_json::Value) {
    for event in Event::from_block(height, block) {
        events.publish(event);
    }
}

#[tokio::test]
async fn a_matching_event_is_delivered_signed_and_others_are_not() {
    let (events, recorder, app) = setup(0);
    let (_, created) = register(&app, "203.0.113.1:1", ALICE).await;
    let secret = created["secret"].as_str().unwrap().to_string();

    publish(&events, 1, &transfer(1, BOB, BOB));
    publish(&events, 2, &transfer(2, BOB, ALICE));
    // Two events at height 2 match (the block and its transaction).
    wait_for(|| recorder.delivered.lock().unwrap().len() == 2).await;
    tokio::time::sleep(Duration::from_millis(30)).await;

    let delivered = recorder.delivered.lock().unwrap();
    assert_eq!(delivered.len(), 2);
    for (body, signature) in delivered.iter() {
        assert!(webhooks::verify(&secret, body, signature));
        let json: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(json["event"]["height"], 2);
        assert_eq!(json["subscription"], created["id"]);
    }
}

#[tokio::test]
async fn a_failing_receiver_is_retried_with_backoff() {
    let (events, recorder, app) = setup(2);
    let (_, created) = register(&app, "203.0.113.1:1", ALICE).await;
    let block = serde_json::json!({ "height": 3, "header": {}, "transactions": [{"txid":"ab","sender":ALICE,"outputs":[]}] });
    // Only the transaction event: filter the block event out by sending it alone.
    let tx = Event::from_block(3, &block)
        .into_iter()
        .find(|e| e.id.starts_with("tx:"))
        .unwrap();
    events.publish(tx);
    wait_for(|| recorder.delivered.lock().unwrap().len() == 1).await;
    assert_eq!(*recorder.attempts.lock().unwrap(), 3);

    let id = created["id"].as_str().unwrap();
    let secret = created["secret"].as_str().unwrap();
    let (_, shown) = call(
        &app,
        "GET",
        &format!("/v1/webhooks/{id}"),
        "203.0.113.1:1",
        Some(secret),
        String::new(),
    )
    .await;
    assert_eq!(shown["state"], "active");
    assert_eq!(shown["delivered"], 1);
    assert_eq!(shown["failed_attempts"], 2);
}

#[tokio::test]
async fn a_receiver_that_never_answers_is_dead_lettered() {
    let (events, recorder, app) = setup(usize::MAX);
    let (_, created) = register(&app, "203.0.113.1:1", ALICE).await;
    let id = created["id"].as_str().unwrap().to_string();
    let secret = created["secret"].as_str().unwrap().to_string();
    publish(&events, 4, &transfer(4, ALICE, BOB));

    let uri = format!("/v1/webhooks/{id}");
    let mut shown = serde_json::Value::Null;
    for _ in 0..200 {
        shown = call(
            &app,
            "GET",
            &uri,
            "203.0.113.1:1",
            Some(&secret),
            String::new(),
        )
        .await
        .1;
        if shown["state"] == "dead_letter" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(shown["state"], "dead_letter", "{shown}");
    assert_eq!(shown["dead_letter"]["event"], "block:4");
    assert!(shown["last_error"].as_str().unwrap().contains("503"));
    // It stops: no attempts after the dead letter, even for new events.
    let attempts = *recorder.attempts.lock().unwrap();
    assert_eq!(attempts, 3);
    publish(&events, 5, &transfer(5, ALICE, BOB));
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert_eq!(*recorder.attempts.lock().unwrap(), attempts);
}

#[test]
fn the_caller_is_read_the_way_the_rate_limiter_reads_it() {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("cf-connecting-ip", "203.0.113.7".parse().unwrap());
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(ConnectInfo::<SocketAddr>("127.0.0.1:1".parse().unwrap()));
    let via_cf = maya_api_gateway::limit::client_address(
        maya_api_gateway::limit::ClientIp::CfConnectingIp,
        &headers,
        &extensions,
    );
    assert_eq!(via_cf, Some("203.0.113.7".parse::<IpAddr>().unwrap()));
    let via_socket = maya_api_gateway::limit::client_address(
        maya_api_gateway::limit::ClientIp::Socket,
        &headers,
        &extensions,
    );
    assert_eq!(via_socket, Some("127.0.0.1".parse::<IpAddr>().unwrap()));
}
