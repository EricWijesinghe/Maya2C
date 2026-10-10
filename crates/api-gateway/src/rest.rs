//! REST routes.
//!
//! One route per allowlisted node method, plus sealed submission and health.
//! Nothing here reaches the node directly — every call goes through
//! [`NodeClient`], which enforces the allowlist at the single point all
//! outbound traffic passes.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Router, http::StatusCode};
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};

use crate::error::GatewayError;
use crate::jsonrpc::{self, RpcRequest};
use crate::node::{Balance, ChainInfo, FeeInfo, NodeClient, Supply};
use crate::sealed::{self, SealedAccepted, SealedSubmission};

/// Shared handler state.
pub struct AppState {
    /// The node this gateway serves.
    pub node: Arc<dyn NodeClient>,
}

/// Length of a hex-encoded 32-byte address.
const ADDRESS_HEX_LEN: usize = 64;

/// A submitted signed transaction.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct RawTransaction {
    /// Hex-encoded signed transaction.
    pub raw: String,
}

/// The hash of an accepted transaction.
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct TransactionAccepted {
    /// Hex-encoded transaction hash.
    pub hash: String,
}

/// The `OpenAPI` document for the REST surface.
///
/// # Only the gateway is described here
///
/// The node's own interface is JSON-RPC: one endpoint, the method named in the
/// request body. `OpenAPI` has no way to express that beyond "POST / with a
/// tagged union", which generates a client with one untyped function. The
/// gateway is both the surface `OpenAPI` fits and the surface external
/// developers are meant to call -- its allowlist is what excludes
/// `get_mining_candidate` and `submit_block` from being reachable at all.
///
/// `tests/openapi_tests.rs` asserts every route the router registers appears
/// here. A spec that silently omits a route is worse than no spec: the
/// generated client simply lacks the method, and the gap surfaces months later
/// as a function nobody can find.
#[derive(OpenApi)]
#[openapi(
    paths(health, account, block, supply, fees, chain_info, submit, submit_sealed, jsonrpc::handle),
    components(schemas(
        RawTransaction,
        TransactionAccepted,
        Balance,
        FeeInfo,
        ChainInfo,
        Supply,
        RpcRequest,
        SealedSubmission,
        SealedAccepted,
    )),
    tags(
        (name = "chain", description = "Chain reads and transaction submission"),
        (name = "gateway", description = "The gateway's own liveness"),
    ),
    info(
        title = "Maya2C API gateway",
        description = "REST surface for the Maya2C network. Signing happens client-side; \
                       see the sdk-wasm crate.",
    ),
)]
pub struct ApiDoc;

/// Builds the REST router.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/accounts/{address}", get(account))
        .route("/v1/blocks/{height}", get(block))
        .route("/v1/supply", get(supply))
        .route("/v1/fees", get(fees))
        .route("/v1/chain", get(chain_info))
        .route("/v1/transactions", post(submit))
        .route("/v1/sealed", post(submit_sealed))
        // JSON-RPC for clients that speak the node's protocol. Same node
        // client, same allowlist; see `jsonrpc` for why it is not a proxy.
        .route("/rpc", post(jsonrpc::handle))
        // Served rather than only generated: a spec that lives in the repo and
        // not at the endpoint is a spec that drifts from the deployment
        // somebody is actually pointing a client at.
        .route("/openapi.json", get(openapi))
        .with_state(state)
}

/// The `OpenAPI` document, as JSON.
async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

/// Liveness. Deliberately does not touch the node.
///
/// A health check that proxied upstream would report the gateway as unhealthy
/// whenever the node was, and an orchestrator would then restart a process that
/// is working perfectly — losing the one component still able to return a
/// useful error.
#[utoipa::path(
    get,
    path = "/health",
    tag = "gateway",
    responses((status = 200, description = "The gateway is serving. Says nothing about the node.")),
)]
async fn health() -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" })))
}

/// Validates a hex address before it is forwarded.
///
/// The node would reject a malformed address too. Doing it here means a typo
/// costs a local string check rather than a round trip, and it keeps
/// obviously-bad input from reaching the backend at all.
pub(crate) fn parse_address(address: &str) -> Result<&str, GatewayError> {
    let trimmed = address.strip_prefix("0x").unwrap_or(address);
    if trimmed.len() != ADDRESS_HEX_LEN {
        return Err(GatewayError::BadRequest(format!(
            "address must be {ADDRESS_HEX_LEN} hex characters, got {}",
            trimmed.len()
        )));
    }
    if !trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(GatewayError::BadRequest(
            "address contains a non-hex character".to_string(),
        ));
    }
    Ok(trimmed)
}

#[utoipa::path(
    get,
    path = "/v1/accounts/{address}",
    tag = "chain",
    params(("address" = String, Path, description = "Hex-encoded 32-byte address, with or without a 0x prefix")),
    responses(
        (status = 200, description = "Balance and next nonce", body = Balance),
        (status = 400, description = "The address is not 64 hex characters"),
        (status = 502, description = "The node could not be reached"),
    ),
)]
async fn account(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> Result<Json<Balance>, GatewayError> {
    let address = parse_address(&address)?;
    Ok(Json(state.node.get_balance(address).await?))
}

#[utoipa::path(
    get,
    path = "/v1/blocks/{height}",
    tag = "chain",
    params(("height" = u64, Path, description = "Block height")),
    responses(
        (status = 200, description = "The block, in the node's own JSON shape"),
        (status = 404, description = "No block at that height"),
        (status = 502, description = "The node could not be reached"),
    ),
)]
async fn block(
    State(state): State<Arc<AppState>>,
    Path(height): Path<u64>,
) -> Result<Json<serde_json::Value>, GatewayError> {
    let block = state.node.get_block_by_height(height).await?;
    if block.is_null() {
        return Err(GatewayError::NotFound);
    }
    Ok(Json(block))
}

#[utoipa::path(
    get,
    path = "/v1/supply",
    tag = "chain",
    responses(
        (status = 200, description = "Circulating and total supply, in base units", body = Supply),
        (status = 502, description = "The node could not be reached"),
    ),
)]
async fn supply(State(state): State<Arc<AppState>>) -> Result<Json<Supply>, GatewayError> {
    Ok(Json(state.node.get_supply().await?))
}

#[utoipa::path(
    get,
    path = "/v1/fees",
    tag = "chain",
    responses(
        (status = 200, description = "Fee terms (ADR-029): a transfer pays at least base_fee per                                       serialized byte, as an output to the collector", body = FeeInfo),
        (status = 502, description = "The node could not be reached"),
    ),
)]
async fn fees(State(state): State<Arc<AppState>>) -> Result<Json<FeeInfo>, GatewayError> {
    Ok(Json(state.node.get_fee_info().await?))
}

#[utoipa::path(
    get,
    path = "/v1/chain",
    tag = "chain",
    responses(
        (status = 200, description = "Chain identification: genesis block id and optional chain id string. Needed for offline signing (ADR-036)", body = ChainInfo),
        (status = 502, description = "The node could not be reached"),
    ),
)]
async fn chain_info(State(state): State<Arc<AppState>>) -> Result<Json<ChainInfo>, GatewayError> {
    Ok(Json(state.node.get_chain_info().await?))
}

#[utoipa::path(
    post,
    path = "/v1/transactions",
    tag = "chain",
    request_body = RawTransaction,
    responses(
        (status = 200, description = "Accepted into the node's mempool. Not inclusion in a block.", body = TransactionAccepted),
        (status = 400, description = "The body was empty or not hex"),
        (status = 502, description = "The node refused it or could not be reached"),
    ),
)]
async fn submit(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RawTransaction>,
) -> Result<Json<TransactionAccepted>, GatewayError> {
    if body.raw.is_empty() {
        return Err(GatewayError::BadRequest("raw is empty".to_string()));
    }
    let hash = state.node.send_raw_transaction(&body.raw).await?;
    Ok(Json(TransactionAccepted { hash }))
}

/// Accepts a threshold-encrypted transaction.
///
/// The gateway validates shape and forwards. It never decrypts — see
/// [`crate::sealed`].
#[utoipa::path(
    post,
    path = "/v1/sealed",
    tag = "chain",
    request_body = SealedSubmission,
    responses(
        (status = 200, description = "Accepted. A sealed submission is an ordinary transaction to the node.", body = SealedAccepted),
        (status = 400, description = "The envelope failed validation"),
        (status = 502, description = "The node could not be reached"),
    ),
)]
async fn submit_sealed(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SealedSubmission>,
) -> Result<Json<SealedAccepted>, GatewayError> {
    let bytes = sealed::validate(&body)?;

    // Forwarded on the ordinary transaction path: to the node, a sealed
    // transaction is a transaction whose payload it cannot read, and the
    // gateway has no separate channel for one.
    let id = state.node.send_raw_transaction(&body.ciphertext).await?;

    Ok(Json(SealedAccepted {
        id,
        bytes: bytes.len(),
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn an_address_may_carry_an_optional_prefix() {
        let bare = "a".repeat(ADDRESS_HEX_LEN);
        let prefixed = format!("0x{bare}");
        assert_eq!(parse_address(&bare).expect("bare"), bare);
        assert_eq!(parse_address(&prefixed).expect("prefixed"), bare);
    }

    #[test]
    fn a_short_or_long_address_is_refused() {
        assert!(parse_address(&"a".repeat(ADDRESS_HEX_LEN - 1)).is_err());
        assert!(parse_address(&"a".repeat(ADDRESS_HEX_LEN + 1)).is_err());
    }

    #[test]
    fn a_non_hex_address_is_refused() {
        let bad = format!("{}z", "a".repeat(ADDRESS_HEX_LEN - 1));
        assert!(parse_address(&bad).is_err());
    }
}
