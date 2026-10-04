//! JSON-RPC 2.0 at `POST /rpc`, for clients that speak the node's protocol.
//!
//! # Why the gateway speaks JSON-RPC at all
//!
//! `l1-wallet`, the desktop wallet and the Go and Python SDKs are JSON-RPC
//! clients of a node. Without this endpoint none of them could use the public
//! gateway: every user would need a node of their own, or the node's own
//! port would have to be published, miner interface and all.
//!
//! # Not a proxy
//!
//! Requests are not forwarded as JSON. Each method name is matched here and
//! dispatched to its typed [`NodeClient`] method, and its params are parsed
//! and validated before that call. So the allowlist stays a boundary with the
//! same shape as REST: a method this module does not name cannot reach the
//! node, whatever the allowlist says, and malformed params cost a local check,
//! not a round trip.
//!
//! Batches are refused: one request may not fan out into many node calls.
//! Errors keep the REST policy — [`GatewayError::public_message`], never the
//! node's own text.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use serde_json::{Value, json};
use utoipa::ToSchema;

use crate::error::GatewayError;
use crate::node::NodeClient;
use crate::rest::{AppState, parse_address};

const PARSE_ERROR: i64 = -32_700;
const INVALID_REQUEST: i64 = -32_600;
const METHOD_NOT_FOUND: i64 = -32_601;
const INVALID_PARAMS: i64 = -32_602;
const INTERNAL_ERROR: i64 = -32_603;
/// The node's own refusal code, which clients already treat as "refused".
const REFUSED: i64 = -32_000;

/// The shape of a request to `/rpc`, for the `OpenAPI` document.
#[derive(ToSchema)]
#[allow(dead_code)] // Documents the wire shape; requests are parsed as `Value`.
pub struct RpcRequest {
    /// Always `"2.0"`.
    jsonrpc: String,
    /// Echoed in the response.
    #[schema(value_type = Object)]
    id: Value,
    /// One of `get_balance`, `get_block_by_height`, `get_fee_info`,
    /// `get_chain_info`, `get_supply`, `get_bft_status`, `get_checkpoint`,
    /// `send_raw_transaction`.
    method: String,
    /// Positional parameters.
    #[schema(value_type = Vec<Object>)]
    params: Vec<Value>,
}

/// A JSON-RPC error: code and client-visible message.
struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<GatewayError> for RpcError {
    fn from(error: GatewayError) -> Self {
        // Logged in full, returned in part, as for REST.
        let code = match &error {
            GatewayError::MethodNotAllowed(_) => METHOD_NOT_FOUND,
            GatewayError::BadRequest(_) | GatewayError::PayloadTooLarge(_) => INVALID_PARAMS,
            GatewayError::Rejected(_) | GatewayError::NotFound => {
                tracing::info!(error = %error, "request refused by the node");
                REFUSED
            }
            GatewayError::Upstream(_) => {
                tracing::warn!(error = %error, "upstream failure");
                INTERNAL_ERROR
            }
        };
        Self::new(code, error.public_message())
    }
}

/// `POST /rpc`.
#[utoipa::path(
    post,
    path = "/rpc",
    tag = "chain",
    request_body = RpcRequest,
    responses(
        (status = 200, description = "A JSON-RPC 2.0 response: `result`, or `error` with a code. \
                                      Methods: get_balance, get_block_by_height, get_fee_info, \
                                      get_supply, send_raw_transaction. Batches are refused."),
    ),
)]
pub async fn handle(State(state): State<Arc<AppState>>, body: Bytes) -> Json<Value> {
    let request: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return Json(reply(
                &Value::Null,
                Err(RpcError::new(PARSE_ERROR, "not JSON")),
            ));
        }
    };
    let (id, method, params) = match parse(&request) {
        Ok(parts) => parts,
        Err((id, error)) => return Json(reply(&id, Err(error))),
    };
    let outcome = dispatch(state.node.as_ref(), method, params).await;
    Json(reply(&id, outcome))
}

fn reply(id: &Value, outcome: Result<Value, RpcError>) -> Value {
    match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": error.code, "message": error.message },
        }),
    }
}

/// Splits a request into id, method and positional params.
fn parse(request: &Value) -> Result<(Value, &str, &[Value]), (Value, RpcError)> {
    if request.is_array() {
        let error = RpcError::new(INVALID_REQUEST, "batch requests are not supported");
        return Err((Value::Null, error));
    }
    let Some(object) = request.as_object() else {
        return Err((
            Value::Null,
            RpcError::new(INVALID_REQUEST, "not a request object"),
        ));
    };
    let id = object.get("id").cloned().unwrap_or(Value::Null);
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err((
            id,
            RpcError::new(INVALID_REQUEST, "jsonrpc must be \"2.0\""),
        ));
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Err((
            id,
            RpcError::new(INVALID_REQUEST, "method must be a string"),
        ));
    };
    let params = match object.get("params") {
        None => &[][..],
        Some(Value::Array(params)) => params.as_slice(),
        Some(_) => return Err((id, RpcError::new(INVALID_PARAMS, "params must be an array"))),
    };
    Ok((id, method, params))
}

async fn dispatch(
    node: &dyn NodeClient,
    method: &str,
    params: &[Value],
) -> Result<Value, RpcError> {
    match method {
        "get_balance" => get_balance(node, params).await,
        "get_block_by_height" => {
            let [height] = params else {
                return Err(RpcError::new(INVALID_PARAMS, "expected [height]"));
            };
            let height = height
                .as_u64()
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "height must be a whole number"))?;
            Ok(node.get_block_by_height(height).await?)
        }
        "get_supply" => {
            no_params(params)?;
            Ok(to_value(node.get_supply().await?))
        }
        "get_fee_info" => {
            no_params(params)?;
            Ok(to_value(node.get_fee_info().await?))
        }
        "get_chain_info" => {
            no_params(params)?;
            Ok(to_value(node.get_chain_info().await?))
        }
        // Allowlisted with ADR-038 but, until 2026-10-04, never dispatched:
        // the allowlist and this match must both name a method.
        "get_bft_status" => {
            no_params(params)?;
            Ok(node.get_bft_status().await?)
        }
        "get_checkpoint" => {
            no_params(params)?;
            Ok(node.get_checkpoint().await?)
        }
        "send_raw_transaction" => send_raw_transaction(node, params).await,
        _ => Err(RpcError::new(
            METHOD_NOT_FOUND,
            "method not available through the gateway",
        )),
    }
}

/// Answers in the node's `AccountInfo` shape, address included: clients
/// deserialise that type, and a response without the field fails to parse.
async fn get_balance(node: &dyn NodeClient, params: &[Value]) -> Result<Value, RpcError> {
    let [Value::String(address)] = params else {
        return Err(RpcError::new(INVALID_PARAMS, "expected [address]"));
    };
    let address = parse_address(address)?;
    let balance = node.get_balance(address).await?;
    Ok(json!({ "address": address, "balance": balance.balance, "nonce": balance.nonce }))
}

/// `accepted` is `true` for anything the node did not refuse, new or already
/// known: the gateway's node client keeps the txid only, and either way the
/// transaction is in the node's mempool.
async fn send_raw_transaction(node: &dyn NodeClient, params: &[Value]) -> Result<Value, RpcError> {
    let [Value::String(raw)] = params else {
        return Err(RpcError::new(INVALID_PARAMS, "expected [raw_hex]"));
    };
    if raw.is_empty() {
        return Err(RpcError::new(INVALID_PARAMS, "raw is empty"));
    }
    let txid = node.send_raw_transaction(raw).await?;
    Ok(json!({ "txid": txid, "accepted": true }))
}

fn no_params(params: &[Value]) -> Result<(), RpcError> {
    if params.is_empty() {
        Ok(())
    } else {
        Err(RpcError::new(INVALID_PARAMS, "this method takes no params"))
    }
}

fn to_value<T: serde::Serialize>(value: T) -> Value {
    // Serialising a plain struct of numbers and strings cannot fail.
    serde_json::to_value(value).unwrap_or(Value::Null)
}
