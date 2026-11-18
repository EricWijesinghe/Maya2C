mod common;

use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use common::{signed_at, signer};
use http_body_util::BodyExt;
use maya_agent_registry::card::SignedCard;
use maya_agent_registry::http::{AppState, now, router};
use maya_agent_registry::limit::FixedWindow;
use maya_agent_registry::registry::Registry;
use tower::ServiceExt;

const PEER: &str = "127.0.0.1:50000";

fn app(max_per_window: u32) -> Router {
    router(AppState::new(
        Registry::default(),
        FixedWindow::new(3600, max_per_window),
    ))
}

fn request(method: &str, uri: &str, body: Body, peer: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(body)
        .expect("request");
    let addr: SocketAddr = peer.parse().expect("peer address");
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let resp = app.clone().oneshot(req).await.expect("infallible service");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, bytes.to_vec())
}

fn card_json(card: &SignedCard) -> Body {
    Body::from(serde_json::to_vec(card).expect("serialize"))
}

#[tokio::test]
async fn a_signed_card_is_published_and_read_back() {
    let app = app(100);
    let alice = signer(1);
    let at = now();
    let card = signed_at(&alice, "translate", at);
    let id = hex::encode(alice.agent_id());

    let (status, _) = send(&app, request("POST", "/v1/cards", card_json(&card), PEER)).await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, body) = send(
        &app,
        request("GET", &format!("/v1/agents/{id}"), Body::empty(), PEER),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let back: SignedCard = serde_json::from_slice(&body).expect("card json");
    assert_eq!(back, card);

    let (status, body) = send(
        &app,
        request(
            "GET",
            "/v1/agents?capability=translate",
            Body::empty(),
            PEER,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let list: Vec<SignedCard> = serde_json::from_slice(&body).expect("list json");
    assert_eq!(list.len(), 1);
}

#[tokio::test]
async fn a_tampered_card_is_refused_with_a_fixed_message() {
    let app = app(100);
    let mut card = signed_at(&signer(1), "translate", now());
    card.card.name = "impostor".into();

    let (status, body) = send(&app, request("POST", "/v1/cards", card_json(&card), PEER)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("signature does not verify"), "{text}");
}

#[tokio::test]
async fn an_unknown_agent_is_not_found() {
    let app = app(100);
    let id = hex::encode([0u8; 32]);
    let (status, _) = send(
        &app,
        request("GET", &format!("/v1/agents/{id}"), Body::empty(), PEER),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_client_over_its_budget_is_limited_and_others_are_not() {
    let app = app(2);
    let get = || {
        request(
            "GET",
            "/v1/agents?capability=translate",
            Body::empty(),
            PEER,
        )
    };
    assert_eq!(send(&app, get()).await.0, StatusCode::OK);
    assert_eq!(send(&app, get()).await.0, StatusCode::OK);
    assert_eq!(send(&app, get()).await.0, StatusCode::TOO_MANY_REQUESTS);

    let other = request(
        "GET",
        "/v1/agents?capability=translate",
        Body::empty(),
        "10.0.0.9:1",
    );
    assert_eq!(send(&app, other).await.0, StatusCode::OK);
}

#[tokio::test]
async fn a_malformed_body_gets_a_fixed_message_not_the_parser_detail() {
    let app = app(100);
    let body = Body::from(r#"{"card": {"name": "secret-probe-text"}}"#);
    let (status, body) = send(&app, request("POST", "/v1/cards", body, PEER)).await;
    assert!(status.is_client_error(), "{status}");
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("body is not a signed card"), "{text}");
    assert!(!text.contains("missing field"), "{text}");

    let (status, body) = send(&app, request("GET", "/v1/agents", Body::empty(), PEER)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!String::from_utf8_lossy(&body).contains("deserialize"));
}
