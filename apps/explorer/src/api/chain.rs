//! Chain-level and live routes: status, accounts, validators, supply, mempool.

use axum::extract::{Path, State};
use axum::response::Response;
use maya_address::Network;
use serde::Serialize;
use utoipa::ToSchema;

use crate::api::address;
use crate::api::dto::{self, Account, Counts, Status};
use crate::api::node::Mempool;
use crate::api::problem::{ApiError, Code};
use crate::api::respond::{self, Freshness, Meta};
use crate::api::{ApiState, LIVE_TTL, SUPPLY_TTL, chain_meta, finalized_height, network_of};
use crate::error::ExplorerError;
use crate::model::average_block_time;

/// Blocks in the block-time window: the same 30 the dashboard samples.
const BLOCK_TIME_WINDOW: i64 = 30;

/// A store failure, reported without its text: a database error can name
/// tables and hosts, which a public client has no business seeing.
pub fn index_error(error: &ExplorerError) -> ApiError {
    eprintln!("api: store: {error}");
    ApiError::new(Code::IndexBehind, "the index cannot answer right now; retry shortly")
}

/// A node failure. Its text is logged, not returned.
pub fn node_error(error: &ExplorerError) -> ApiError {
    eprintln!("api: node: {error}");
    ApiError::new(Code::NodeUnavailable, "the node cannot answer right now; retry shortly")
}

/// Chain and index summary.
#[utoipa::path(
    get,
    path = "/api/v1/status",
    tag = "chain",
    responses(
        (status = 200, description = "Chain and index summary. Node fields are null while the node is unreachable.", body = Status),
    )
)]
pub async fn status(State(state): State<ApiState>) -> Result<Response, ApiError> {
    let info = chain_meta(&state).await;
    let finalized = finalized_height(&state).await;
    let network = info
        .as_ref()
        .and_then(|i| i.chain_id.as_deref())
        .and_then(Network::for_chain_id);

    let indexed = state.store.latest_height().await.map_err(|e| index_error(&e))?;
    let oldest = state.store.oldest_height().await.map_err(|e| index_error(&e))?;
    let window = state
        .store
        .latest_blocks(BLOCK_TIME_WINDOW)
        .await
        .map_err(|e| index_error(&e))?;
    let counts = Counts {
        blocks: state.store.block_count().await.map_err(|e| index_error(&e))?,
        transactions: state
            .store
            .transaction_count()
            .await
            .map_err(|e| index_error(&e))?,
    };

    let tip = info.as_ref().map(|i| i.height);
    let lag = match (tip, indexed.and_then(|h| u64::try_from(h).ok())) {
        (Some(tip), Some(indexed)) => Some(tip.saturating_sub(indexed)),
        _ => None,
    };
    let body = Status {
        chain_id: info.as_ref().and_then(|i| i.chain_id.clone()),
        network: network.map(|n| n.hrp().to_string()),
        indexed_height: indexed,
        oldest_indexed_height: oldest,
        tip_height: tip,
        finalized_height: finalized,
        lag_blocks: lag,
        counts,
        average_block_time_seconds: average_block_time(&window),
    };
    let meta = Meta {
        indexed,
        tip,
        cache: None,
    };
    Ok(respond::json(&body, Freshness::Live, meta))
}

/// Live balance and next nonce of an account.
#[utoipa::path(
    get,
    path = "/api/v1/accounts/{address}",
    tag = "accounts",
    params(("address" = String, Path, description = "Bech32m address (ADR-081)")),
    responses(
        (status = 200, description = "The account now, read from the node.", body = Account),
        (status = 400, description = "invalid_address or wrong_network", body = crate::api::openapi::Problem),
        (status = 503, description = "node_unavailable", body = crate::api::openapi::Problem),
    )
)]
pub async fn account(
    State(state): State<ApiState>,
    Path(raw): Path<String>,
) -> Result<Response, ApiError> {
    let id = address::parse(&raw, network_of(&state).await)?;
    let id_hex = hex::encode(id);
    let fetch = || async { state.node.account(&id_hex).await };
    let (info, source) = state
        .live
        .account
        .get_or_fetch(&id_hex, LIVE_TTL, fetch)
        .await
        .map_err(|e| node_error(&e))?;
    let body = Account {
        address: raw,
        balance: info.balance.to_string(),
        nonce: info.nonce,
    };
    let meta = Meta {
        cache: Some(source),
        ..Meta::default()
    };
    Ok(respond::json(&body, Freshness::Live, meta))
}

/// The validator set. `404 no_staking` on a chain without staking.
#[utoipa::path(
    get,
    path = "/api/v1/validators",
    tag = "chain",
    responses(
        (status = 200, description = "The committee and validator list, as the node reports them.", body = Object),
        (status = 404, description = "no_staking", body = crate::api::openapi::Problem),
        (status = 503, description = "node_unavailable", body = crate::api::openapi::Problem),
    )
)]
pub async fn validators(State(state): State<ApiState>) -> Result<Response, ApiError> {
    let fetch = || async { state.node.validators().await };
    let (value, source) = state
        .live
        .validators
        .get_or_fetch("validators", LIVE_TTL, fetch)
        .await
        .map_err(|e| node_error(&e))?;
    let meta = Meta {
        cache: Some(source),
        ..Meta::default()
    };
    match value {
        Some(info) => Ok(respond::json(&info, Freshness::Live, meta)),
        None => Err(ApiError::new(
            Code::NoStaking,
            "this network has no staking, so it has no validator set",
        )),
    }
}
/// Total and circulating supply. Cached 30 s: the node scans every account.
#[utoipa::path(
    get,
    path = "/api/v1/supply",
    tag = "chain",
    responses(
        (status = 200, description = "Supply figures.", body = dto::Supply),
        (status = 503, description = "node_unavailable", body = crate::api::openapi::Problem),
    )
)]
pub async fn supply(State(state): State<ApiState>) -> Result<Response, ApiError> {
    let fetch = || async { state.node.supply().await };
    let (report, source) = state
        .live
        .supply
        .get_or_fetch("supply", SUPPLY_TTL, fetch)
        .await
        .map_err(|e| node_error(&e))?;
    let body = dto::Supply {
        total: report.total.to_string(),
        circulating: report.circulating.to_string(),
    };
    let meta = Meta {
        cache: Some(source),
        ..Meta::default()
    };
    Ok(respond::json(&body, Freshness::Live, meta))
}

/// Mempool occupancy: size and bytes, nothing else.
#[utoipa::path(
    get,
    path = "/api/v1/mempool",
    tag = "chain",
    responses(
        (status = 200, description = "Mempool occupancy.", body = dto::Mempool),
        (status = 503, description = "node_unavailable", body = crate::api::openapi::Problem),
    )
)]
pub async fn mempool(State(state): State<ApiState>) -> Result<Response, ApiError> {
    let fetch = || async { state.node.mempool().await };
    let (pool, source) = state
        .live
        .mempool
        .get_or_fetch("mempool", LIVE_TTL, fetch)
        .await
        .map_err(|e| node_error(&e))?;
    let body = dto::Mempool::from(pool);
    let meta = Meta {
        cache: Some(source),
        ..Meta::default()
    };
    Ok(respond::json(&body, Freshness::Live, meta))
}

impl From<Mempool> for dto::Mempool {
    fn from(pool: Mempool) -> Self {
        Self {
            size: pool.size,
            bytes: pool.bytes,
        }
    }
}
