//! The HTTP registry. Every card is verified on the way in; clients (the MCP
//! tools) verify again on the way out, since they cannot trust this server.
//!
//! Routes: `POST /v1/cards`, `GET /v1/agents?capability=`, `GET /v1/agents/{id}`.
//! Errors carry a fixed message from `Error`, never a parser's detail: body and
//! query rejections are mapped to fixed strings too.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::Router;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::json;

use crate::card::{MAX_TEXT_BYTES, SignedCard};
use crate::error::Error;
use crate::limit::{FixedWindow, client_key};
use crate::registry::Registry;
use crate::wire;

/// Largest request body. A hybrid signature is 11,165 bytes and the key 1,984,
/// so a card is about 26 KiB once both are hex.
pub const MAX_BODY_BYTES: usize = 64 * 1024;
/// Most cards returned by one search.
pub const MAX_RESULTS: usize = 100;

pub struct Shared {
    pub registry: Registry,
    pub limiter: FixedWindow,
}

#[derive(Clone)]
pub struct AppState(Arc<Mutex<Shared>>);

impl AppState {
    pub fn new(registry: Registry, limiter: FixedWindow) -> Self {
        Self(Arc::new(Mutex::new(Shared { registry, limiter })))
    }

    fn lock(&self) -> Result<MutexGuard<'_, Shared>, StatusCode> {
        self.0.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    }
}

/// Unix seconds. A clock that cannot be read gives `u64::MAX`, which expires
/// every card: the registry fails closed.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(u64::MAX, |d| d.as_secs())
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/cards", post(publish))
        .route("/v1/agents", get(search))
        .route("/v1/agents/{id}", get(get_one))
        .layer(middleware::from_fn_with_state(state.clone(), limit))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

/// Counts every request against its client address before the handler runs.
async fn limit(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let admitted = match state.lock() {
        Ok(mut shared) => shared.limiter.admit(&client_key(peer.ip()), now()),
        Err(code) => return code.into_response(),
    };
    if admitted {
        next.run(request).await
    } else {
        reply(StatusCode::TOO_MANY_REQUESTS, "rate limited")
    }
}

fn reply(code: StatusCode, message: &str) -> Response {
    (code, Json(json!({"error": message}))).into_response()
}

fn error_response(error: Error) -> Response {
    let code = match error {
        Error::Full => StatusCode::SERVICE_UNAVAILABLE,
        Error::Stale | Error::Replay => StatusCode::CONFLICT,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    reply(code, &error.to_string())
}

async fn publish(
    State(state): State<AppState>,
    body: Result<Json<SignedCard>, JsonRejection>,
) -> Response {
    let card = match body {
        Ok(Json(card)) => card,
        Err(rejection) => return reply(rejection.status(), "body is not a signed card"),
    };
    let outcome = match state.lock() {
        Ok(mut shared) => shared.registry.publish(card, now()),
        Err(code) => return code.into_response(),
    };
    match outcome {
        Ok(id) => (
            StatusCode::CREATED,
            Json(json!({"agent_id": hex::encode(id)})),
        )
            .into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    capability: String,
}

async fn search(
    State(state): State<AppState>,
    query: Result<Query<SearchParams>, QueryRejection>,
) -> Response {
    let Ok(Query(params)) = query else {
        return reply(StatusCode::BAD_REQUEST, "capability is required");
    };
    if params.capability.is_empty() || params.capability.len() > MAX_TEXT_BYTES {
        return reply(StatusCode::BAD_REQUEST, "capability out of bounds");
    }
    let cards: Vec<SignedCard> = match state.lock() {
        Ok(shared) => shared
            .registry
            .search(&params.capability, now())
            .into_iter()
            .take(MAX_RESULTS)
            .cloned()
            .collect(),
        Err(code) => return code.into_response(),
    };
    Json(cards).into_response()
}

async fn get_one(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(id) = wire::hex32_from_str(&id) else {
        return reply(StatusCode::BAD_REQUEST, "agent id must be 32 bytes of hex");
    };
    let found = match state.lock() {
        Ok(shared) => shared.registry.get(&id, now()).cloned(),
        Err(code) => return code.into_response(),
    };
    match found {
        Some(card) => Json(card).into_response(),
        None => reply(StatusCode::NOT_FOUND, "not found"),
    }
}
