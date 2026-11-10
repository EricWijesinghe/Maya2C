//! Public indexer API, v1 (ADR-092), mounted at `/api/v1`.
//!
//! History comes from the store. Live values (balance, validators, supply,
//! mempool, chain identity, finality) come from the node through
//! [`cache::TtlCache`]. Every route is read-only GET and rate limited per
//! client. The OpenAPI document in [`openapi`] is the contract; the golden file
//! `apps/explorer/openapi/v1.yaml` pins it.

pub mod address;
pub mod cache;
pub mod chain;
pub mod dto;
pub mod history;
pub mod node;
pub mod openapi;
pub mod problem;
pub mod query;
pub mod respond;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::Response;
use axum::routing::get;
use custom_l1_node::rpc::types::CheckpointInfo;
use custom_l1_node::rpc::validators::ValidatorsInfo;
use custom_l1_node::rpc::{AccountInfo, ChainInfo, SupplyReport};
use maya_web_security::rate_limit::{self, InvalidRateLimit, RateLimit};
use tower_governor::{GovernorError, GovernorLayer};

use crate::api::cache::TtlCache;
use crate::api::node::{Mempool, NodeSource};
use crate::api::problem::{Code, problem_response};
use crate::store::BlockStore;

/// How long live node answers are reused. Short: a wallet should see a new
/// balance within a block or two, and the cache exists for bursts, not for
/// staleness.
pub const LIVE_TTL: Duration = Duration::from_secs(2);

/// `get_supply` scans every account on the node, so it is reused for longer.
pub const SUPPLY_TTL: Duration = Duration::from_secs(30);

/// Every route the router serves. The OpenAPI test checks the spec against it.
pub const ROUTES: &[&str] = &[
    "/api/v1/status",
    "/api/v1/blocks",
    "/api/v1/blocks/{reference}",
    "/api/v1/blocks/{reference}/transactions",
    "/api/v1/transactions",
    "/api/v1/transactions/{txid}",
    "/api/v1/accounts/{address}",
    "/api/v1/accounts/{address}/transactions",
    "/api/v1/validators",
    "/api/v1/supply",
    "/api/v1/mempool",
];

/// Default per-client limit: 10 requests a second, bursts of 30.
#[must_use]
pub const fn default_rate_limit() -> RateLimit {
    RateLimit {
        per_second: 10,
        burst: 30,
        client_ip: rate_limit::ClientIp::Socket,
    }
}

/// The per-process caches for live node data.
#[derive(Default)]
pub struct LiveCache {
    /// `get_chain_info`, under one key.
    pub chain: TtlCache<ChainInfo>,
    /// `get_checkpoint`, under one key.
    pub checkpoint: TtlCache<Option<CheckpointInfo>>,
    /// `get_balance`, keyed by hex address.
    pub account: TtlCache<AccountInfo>,
    /// `get_validators`, under one key.
    pub validators: TtlCache<Option<ValidatorsInfo>>,
    /// `get_supply`, under one key.
    pub supply: TtlCache<SupplyReport>,
    /// `get_mempool_info`, under one key.
    pub mempool: TtlCache<Mempool>,
}

/// What every v1 handler needs.
#[derive(Clone)]
pub struct ApiState {
    /// Indexed history.
    pub store: Arc<dyn BlockStore>,
    /// The node, for live values.
    pub node: Arc<dyn NodeSource>,
    /// Caches for [`ApiState::node`].
    pub live: Arc<LiveCache>,
}

/// Builds the v1 router, generic over the outer application's state so it can
/// be merged into a router that carries its own.
///
/// # Errors
///
/// [`InvalidRateLimit`] if `limit` allows nothing.
pub fn router<S>(state: ApiState, limit: &RateLimit) -> Result<Router<S>, InvalidRateLimit> {
    let config = rate_limit::config(limit)?;
    // Forgotten clients are pruned on a timer; without it the table grows with
    // every address that ever called.
    drop(rate_limit::spawn_pruning(&config));
    let burst = limit.burst;
    let router = Router::new()
        .route("/api/v1/status", get(chain::status))
        .route("/api/v1/blocks", get(history::blocks))
        .route("/api/v1/blocks/{reference}", get(history::block))
        .route(
            "/api/v1/blocks/{reference}/transactions",
            get(history::block_transactions),
        )
        .route("/api/v1/transactions", get(history::transactions))
        .route("/api/v1/transactions/{txid}", get(history::transaction))
        .route("/api/v1/accounts/{address}", get(chain::account))
        .route(
            "/api/v1/accounts/{address}/transactions",
            get(history::account_transactions),
        )
        .route("/api/v1/validators", get(chain::validators))
        .route("/api/v1/supply", get(chain::supply))
        .route("/api/v1/mempool", get(chain::mempool))
        .layer(axum::middleware::from_fn(respond::conditional))
        // Outermost, so a limited client costs nothing beyond this check.
        .layer(GovernorLayer::new(config).error_handler(move |error| rate_limited(burst, error)))
        .with_state(state);
    Ok(router)
}

/// The 429 a limited client gets: problem JSON, with the retry headers.
fn rate_limited(burst: u32, error: GovernorError) -> Response {
    match error {
        GovernorError::TooManyRequests { wait_time, .. } => {
            let mut headers = HeaderMap::new();
            headers.insert(axum::http::header::RETRY_AFTER, HeaderValue::from(wait_time));
            headers.insert(RATELIMIT_LIMIT, HeaderValue::from(burst));
            headers.insert(RATELIMIT_REMAINING, HeaderValue::from(0u32));
            headers.insert(RATELIMIT_RESET, HeaderValue::from(wait_time));
            let detail = format!("too many requests; retry after {wait_time} s");
            problem_response(Code::RateLimited, &detail, headers)
        }
        other => Response::from(other),
    }
}

/// IETF `RateLimit-*` fields (draft-ietf-httpapi-ratelimit-headers).
pub const RATELIMIT_LIMIT: HeaderName = HeaderName::from_static("ratelimit-limit");
/// Requests left in the window.
pub const RATELIMIT_REMAINING: HeaderName = HeaderName::from_static("ratelimit-remaining");
/// Seconds until the window resets.
pub const RATELIMIT_RESET: HeaderName = HeaderName::from_static("ratelimit-reset");

/// The chain's identity and the node's tip, if it answers. Never an error:
/// history must keep working while the node is down.
pub async fn chain_meta(state: &ApiState) -> Option<ChainInfo> {
    let fetch = || async { state.node.chain().await };
    state
        .live
        .chain
        .get_or_fetch("chain", LIVE_TTL, fetch)
        .await
        .ok()
        .map(|(info, _)| info)
}

/// The network the node serves, once its chain id is known and registered.
pub async fn network_of(state: &ApiState) -> Option<maya_address::Network> {
    let info = chain_meta(state).await?;
    maya_address::Network::for_chain_id(info.chain_id.as_deref()?)
}

/// The finalized height, if the chain has a finality checkpoint.
pub async fn finalized_height(state: &ApiState) -> Option<u64> {
    let fetch = || async { state.node.checkpoint().await };
    state
        .live
        .checkpoint
        .get_or_fetch("checkpoint", LIVE_TTL, fetch)
        .await
        .ok()
        .and_then(|(checkpoint, _)| checkpoint.map(|c| c.height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_limit_matches_the_documented_defaults() {
        let limit = default_rate_limit();
        assert_eq!((limit.per_second, limit.burst), (10, 30));
    }
}
