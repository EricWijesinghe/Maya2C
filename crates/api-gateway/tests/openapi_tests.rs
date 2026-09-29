//! The OpenAPI document and the router must describe the same API.
//!
//! `src/rest.rs` promises this file exists. It did not, for a while: the doc
//! comment was written with the annotations and the test was left for later.
//! A spec that silently omits a route is worse than no spec — the generated
//! client simply lacks the method, and the gap surfaces months later as a
//! function nobody can find.
//!
//! An axum `Router` cannot list its routes, so the registrations are read from
//! the source of `rest.rs` itself. That is the right place to read them from:
//! the failure this catches is somebody adding a `.route(...)` line and not the
//! `#[utoipa::path]` beside it, and that is a fact about the source.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use maya_api_gateway::error::GatewayError;
use maya_api_gateway::node::{Balance, ChainInfo, FeeInfo, NodeClient, Supply};
use maya_api_gateway::rest::ApiDoc;
use tower::ServiceExt;
use utoipa::OpenApi;

/// Routes that exist to serve the API's description rather than the API.
const META_ROUTES: &[&str] = &["/openapi.json"];

/// `(method, path)` for every `.route(...)` in `rest.rs`.
fn registered_routes() -> BTreeSet<(String, String)> {
    let source = include_str!("../src/rest.rs");
    let mut routes = BTreeSet::new();
    for line in source.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix(".route(\"") else {
            continue;
        };
        let (path, after) = rest.split_once('"').expect("a quoted path");
        // `, get(handler))` -> `get`; a chained `get(a).post(b)` lists both.
        for method in ["get", "post", "put", "delete", "patch"] {
            if after.contains(&format!("{method}(")) {
                routes.insert((method.to_string(), path.to_string()));
            }
        }
    }
    routes
}

/// `(method, path)` for every operation in the generated document.
fn documented_routes() -> BTreeSet<(String, String)> {
    let spec = serde_json::to_value(ApiDoc::openapi()).expect("serialise the spec");
    let paths = spec["paths"].as_object().expect("paths object");
    paths
        .iter()
        .flat_map(|(path, item)| {
            item.as_object()
                .expect("path item")
                .keys()
                .map(move |method| (method.to_lowercase(), path.clone()))
        })
        .collect()
}

#[test]
fn the_source_scan_finds_the_routes_it_should() {
    // Guards the guard. If the scan found nothing — a formatting change, say —
    // every other assertion here would hold vacuously.
    let routes = registered_routes();
    assert!(routes.contains(&("get".into(), "/health".into())));
    assert!(routes.contains(&("post".into(), "/v1/transactions".into())));
    assert!(routes.len() >= 6, "found only {routes:?}");
}

#[test]
fn every_registered_route_is_documented() {
    let documented = documented_routes();
    let missing: Vec<_> = registered_routes()
        .into_iter()
        .filter(|(_, path)| !META_ROUTES.contains(&path.as_str()))
        .filter(|route| !documented.contains(route))
        .collect();
    assert!(
        missing.is_empty(),
        "routes with no OpenAPI entry: {missing:?}"
    );
}

#[test]
fn every_documented_route_is_registered() {
    // The other direction: a spec entry for a handler that is not routed is a
    // method every generated client has and every call to returns 404.
    let registered = registered_routes();
    let phantom: Vec<_> = documented_routes()
        .into_iter()
        .filter(|route| !registered.contains(route))
        .collect();
    assert!(phantom.is_empty(), "documented but not routed: {phantom:?}");
}

#[test]
fn the_miner_methods_are_absent_from_the_spec() {
    // The allowlist excludes them from the gateway entirely; the spec must not
    // advertise what the gateway refuses to serve.
    let spec = serde_json::to_string(&ApiDoc::openapi()).expect("serialise");
    for method in ["get_mining_candidate", "submit_block"] {
        assert!(!spec.contains(method), "{method} appears in the spec");
    }
}

/// A node that is never supposed to be called.
struct UnreachableNode;

#[async_trait]
impl NodeClient for UnreachableNode {
    async fn get_balance(&self, _address: &str) -> Result<Balance, GatewayError> {
        unreachable!("serving the spec must not touch the node")
    }
    async fn get_block_by_height(&self, _height: u64) -> Result<serde_json::Value, GatewayError> {
        unreachable!("serving the spec must not touch the node")
    }
    async fn get_supply(&self) -> Result<Supply, GatewayError> {
        unreachable!("serving the spec must not touch the node")
    }
    async fn send_raw_transaction(&self, _raw: &str) -> Result<String, GatewayError> {
        unreachable!("serving the spec must not touch the node")
    }
    async fn get_fee_info(&self) -> Result<FeeInfo, GatewayError> {
        unreachable!("serving the spec must not touch the node")
    }
    async fn get_chain_info(&self) -> Result<ChainInfo, GatewayError> {
        unreachable!("serving the spec must not touch the node")
    }
}

#[tokio::test]
async fn the_served_document_is_the_generated_one() {
    // Served rather than only generated, so a client pointed at a deployment
    // gets that deployment's API. It must be byte-for-byte what the code
    // generates, and fetching it must not reach the node.
    let app = maya_api_gateway::app(Arc::new(UnreachableNode));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/openapi.json")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let served: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
    let generated = serde_json::to_value(ApiDoc::openapi()).expect("serialise");
    assert_eq!(served, generated);
}
