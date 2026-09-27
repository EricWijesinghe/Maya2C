//! Mesh (formerly Rosetta) Data API for a `Maya2C` node (Master Prompt 17).
//!
//! Exchanges and custodians integrate against Mesh rather than a chain's own
//! RPC. This serves the Data API — network, block, account — over a node's
//! JSON-RPC; `mesh-cli check:data` is the acceptance test
//! (`scripts/mesh_check.sh`).
//!
//! **The Construction API is not implemented, and cannot be checked as
//! specified.** Mesh's `CurveType` and `SignatureType` enumerations have no
//! post-quantum scheme, so `mesh-cli check:construction`, which signs with
//! keys it generates itself, has no way to produce an ML-DSA or hybrid
//! signature. `reports/17-integrations.md` records this.
//!
//! Historical balance lookup is on: `/account/balance` at a block index is
//! the node's `get_balance_at_height`, which undoes each later block's
//! recorded changes from the current balance. Without it mesh-cli reconciles
//! only at the tip, and a devnet producing blocks as fast as mesh-cli syncs
//! them never lets it get there.

pub mod map;
pub mod node;
pub mod types;

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::node::{Node, NodeError};
use crate::types::{
    AccountIdentifier, BLOCKCHAIN, BlockIdentifier, MESH_VERSION, MeshError, NetworkIdentifier,
    OP_BALANCE_CHANGE, PartialBlockIdentifier, STATUS_SUCCESS,
};

/// The zero address: read at the tip only to learn the tip atomically.
const TIP_PROBE: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Stable error codes, all listed in `/network/options`.
#[derive(Clone, Copy, Debug)]
enum Code {
    WrongNetwork = 1,
    NotFound = 2,
    Unavailable = 3,
    Unsupported = 4,
}

impl Code {
    fn error(self) -> MeshError {
        let (message, retriable) = match self {
            Self::WrongNetwork => ("network identifier does not match this node", false),
            Self::NotFound => ("block not found or not kept", false),
            Self::Unavailable => ("node unavailable", true),
            Self::Unsupported => ("not supported by this implementation", false),
        };
        MeshError {
            code: self as u32,
            message: message.to_string(),
            retriable,
        }
    }
}

/// A handler's failure, rendered as a Mesh error with status 500.
pub struct ApiError(MeshError);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, Json(self.0)).into_response()
    }
}

impl From<NodeError> for ApiError {
    fn from(e: NodeError) -> Self {
        tracing::warn!(error = %e, "node read failed");
        Self(match e {
            NodeError::NotFound(_) => Code::NotFound.error(),
            NodeError::Unavailable(_) => Code::Unavailable.error(),
        })
    }
}

type ApiResult = Result<Json<Value>, ApiError>;

/// Shared state.
pub struct Api {
    node: Arc<dyn Node>,
    network: NetworkIdentifier,
}

/// The router: every Data API endpoint this implementation serves.
pub fn router(node: Arc<dyn Node>, network: &str) -> Router {
    let api = Arc::new(Api {
        node,
        network: NetworkIdentifier {
            blockchain: BLOCKCHAIN.to_string(),
            network: network.to_string(),
        },
    });
    Router::new()
        .route("/network/list", post(network_list))
        .route("/network/options", post(network_options))
        .route("/network/status", post(network_status))
        .route("/block", post(block))
        .route("/block/transaction", post(unsupported))
        .route("/account/balance", post(account_balance))
        .route("/mempool", post(mempool))
        .with_state(api)
}

#[derive(Deserialize)]
struct NetworkRequest {
    network_identifier: NetworkIdentifier,
}

impl Api {
    fn check(&self, network: &NetworkIdentifier) -> Result<(), ApiError> {
        if *network == self.network {
            Ok(())
        } else {
            Err(ApiError(Code::WrongNetwork.error()))
        }
    }
}

async fn network_list(State(api): State<Arc<Api>>) -> Json<Value> {
    Json(json!({ "network_identifiers": [api.network] }))
}

async fn network_options(
    State(api): State<Arc<Api>>,
    Json(req): Json<NetworkRequest>,
) -> ApiResult {
    api.check(&req.network_identifier)?;
    let errors: Vec<MeshError> = [
        Code::WrongNetwork,
        Code::NotFound,
        Code::Unavailable,
        Code::Unsupported,
    ]
    .into_iter()
    .map(Code::error)
    .collect();
    Ok(Json(json!({
        "version": { "rosetta_version": MESH_VERSION, "node_version": env!("CARGO_PKG_VERSION") },
        "allow": {
            "operation_statuses": [{ "status": STATUS_SUCCESS, "successful": true }],
            "operation_types": [OP_BALANCE_CHANGE],
            "errors": errors,
            "historical_balance_lookup": true,
            "call_methods": [],
            "balance_exemptions": [],
            "mempool_coins": false
        }
    })))
}

async fn network_status(State(api): State<Arc<Api>>, Json(req): Json<NetworkRequest>) -> ApiResult {
    api.check(&req.network_identifier)?;
    let tip = api.node.account_at_tip(TIP_PROBE).await?;
    let current = api.node.block(tip.height).await?;
    let genesis = api.node.block(0).await?;
    Ok(Json(json!({
        "current_block_identifier": BlockIdentifier { index: tip.height, hash: tip.block_id },
        "current_block_timestamp": current.header.timestamp.saturating_mul(1_000),
        "genesis_block_identifier": BlockIdentifier { index: 0, hash: genesis.header.id },
        "peers": []
    })))
}

#[derive(Deserialize)]
struct BlockRequest {
    network_identifier: NetworkIdentifier,
    block_identifier: PartialBlockIdentifier,
}

async fn block(State(api): State<Arc<Api>>, Json(req): Json<BlockRequest>) -> ApiResult {
    api.check(&req.network_identifier)?;
    let height = match (req.block_identifier.index, &req.block_identifier.hash) {
        (Some(index), _) => index,
        (None, None) => api.node.account_at_tip(TIP_PROBE).await?.height,
        // Lookup by hash alone needs a hash index the node does not serve.
        (None, Some(_)) => return Err(ApiError(Code::Unsupported.error())),
    };
    let node_block = api.node.block(height).await?;
    if let Some(hash) = &req.block_identifier.hash
        && *hash != node_block.header.id
    {
        return Err(ApiError(Code::NotFound.error()));
    }
    let changes = if height == 0 {
        Vec::new()
    } else {
        let kept = api.node.balance_changes(height).await?;
        // A reorg between the two reads would pair one block with another's
        // changes; refuse rather than serve a mismatch.
        if kept.block_id != node_block.header.id {
            return Err(ApiError(Code::Unavailable.error()));
        }
        kept.changes
    };
    Ok(Json(json!({ "block": map::block(&node_block, &changes) })))
}

// Field names are the Mesh specification's.
#[allow(clippy::struct_field_names)]
#[derive(Deserialize)]
struct BalanceRequest {
    network_identifier: NetworkIdentifier,
    account_identifier: AccountIdentifier,
    #[serde(default)]
    block_identifier: Option<PartialBlockIdentifier>,
}

async fn account_balance(
    State(api): State<Arc<Api>>,
    Json(req): Json<BalanceRequest>,
) -> ApiResult {
    api.check(&req.network_identifier)?;
    let address = &req.account_identifier.address;
    let at = match req.block_identifier {
        None => api.node.account_at_tip(address).await?,
        Some(PartialBlockIdentifier {
            index: Some(index),
            hash,
        }) => {
            let at = api.node.balance_at(address, index).await?;
            if hash.is_some_and(|h| h != at.block_id) {
                return Err(ApiError(Code::NotFound.error()));
            }
            at
        }
        // By hash alone needs a hash index the node does not serve.
        Some(_) => return Err(ApiError(Code::Unsupported.error())),
    };
    Ok(Json(json!({
        "block_identifier": BlockIdentifier { index: at.height, hash: at.block_id },
        "balances": [{ "value": at.balance.to_string(), "currency": map::currency() }]
    })))
}

async fn mempool(State(api): State<Arc<Api>>, Json(req): Json<NetworkRequest>) -> ApiResult {
    api.check(&req.network_identifier)?;
    Ok(Json(json!({ "transaction_identifiers": [] })))
}

async fn unsupported() -> ApiError {
    ApiError(Code::Unsupported.error())
}
