//! The public JSON-RPC surface the audit reads, and an HTTP client for it.
//!
//! The trait is the seam: the checks take `&dyn Rpc`, so tests run them against
//! an in-process fake, and the HTTP client only moves JSON. Response shapes
//! follow `crates/node/src/rpc/{server,market,types}.rs`; amounts arrive as
//! base-10 strings (`amount_serde`).

use std::time::Duration;

use maya_amount::Amount;
use serde_json::{Value, json};

use crate::numbers::parse_amount;

/// A balance change one block made to one account, from `get_balance_changes`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Change {
    pub address: [u8; 32],
    pub before: Amount,
    pub after: Amount,
}

/// One transaction output, from `get_block_by_height`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Output {
    pub recipient: [u8; 32],
    pub amount: Amount,
}

/// `get_supply`: the node's own sums, at the tip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SupplyFacts {
    pub total: Amount,
    pub shielded: Amount,
    pub burned: Amount,
}

/// `get_chain_info`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainFacts {
    pub genesis: String,
    pub height: u64,
}

/// Why an RPC call failed. Only `Rejected` is data the node does not have
/// (pruned, unknown height); the other two stop the audit.
#[derive(Debug, thiserror::Error)]
pub enum RpcError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("node rejected the call: {0}")]
    Rejected(String),
    #[error("malformed response to {method}: {detail}")]
    Malformed {
        method: &'static str,
        detail: String,
    },
}

/// The read-only calls the audit makes.
pub trait Rpc {
    /// `get_chain_info`.
    fn chain_info(&self) -> Result<ChainFacts, RpcError>;
    /// `get_supply`.
    fn supply(&self) -> Result<SupplyFacts, RpcError>;
    /// `get_balance` at the tip.
    fn balance(&self, address: &[u8; 32]) -> Result<Amount, RpcError>;
    /// `get_balance_at_height`.
    fn balance_at(&self, address: &[u8; 32], height: u64) -> Result<Amount, RpcError>;
    /// `get_balance_changes` for one height.
    fn balance_changes(&self, height: u64) -> Result<Vec<Change>, RpcError>;
    /// Every transaction output in the block at `height`.
    fn block_outputs(&self, height: u64) -> Result<Vec<Output>, RpcError>;
}

/// JSON-RPC over HTTP(S), one request per call.
pub struct HttpRpc {
    url: String,
    client: reqwest::blocking::Client,
}

/// Generous for a public gateway; a slow answer is still an answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

impl HttpRpc {
    /// Builds a client for `url`.
    ///
    /// # Errors
    ///
    /// Transport when the TLS client cannot be built.
    pub fn new(url: &str) -> Result<Self, RpcError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| RpcError::Transport(e.to_string()))?;
        Ok(Self {
            url: url.to_string(),
            client,
        })
    }

    fn call(&self, method: &'static str, params: Value) -> Result<Value, RpcError> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let response = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .map_err(|e| RpcError::Transport(e.to_string()))?;
        let status = response.status();
        let value: Value = response
            .json()
            .map_err(|e| RpcError::Transport(format!("{method}: HTTP {status}: {e}")))?;
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("error");
            return Err(RpcError::Rejected(format!("{method}: {message}")));
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| RpcError::Malformed {
                method,
                detail: "no result or error".into(),
            })
    }
}

impl Rpc for HttpRpc {
    fn chain_info(&self) -> Result<ChainFacts, RpcError> {
        let v = self.call("get_chain_info", json!([]))?;
        Ok(ChainFacts {
            genesis: text(&v, "genesis", "get_chain_info")?,
            height: uint(&v, "height", "get_chain_info")?,
        })
    }

    fn supply(&self) -> Result<SupplyFacts, RpcError> {
        decode_supply(&self.call("get_supply", json!([]))?)
    }

    fn balance(&self, address: &[u8; 32]) -> Result<Amount, RpcError> {
        let v = self.call("get_balance", json!([hex::encode(address)]))?;
        amount(&v, "balance", "get_balance")
    }

    fn balance_at(&self, address: &[u8; 32], height: u64) -> Result<Amount, RpcError> {
        let v = self.call(
            "get_balance_at_height",
            json!([hex::encode(address), height]),
        )?;
        amount(&v, "balance", "get_balance_at_height")
    }

    fn balance_changes(&self, height: u64) -> Result<Vec<Change>, RpcError> {
        decode_changes(&self.call("get_balance_changes", json!([height]))?)
    }

    fn block_outputs(&self, height: u64) -> Result<Vec<Output>, RpcError> {
        decode_outputs(&self.call("get_block_by_height", json!([height]))?)
    }
}

fn malformed(method: &'static str, detail: impl Into<String>) -> RpcError {
    RpcError::Malformed {
        method,
        detail: detail.into(),
    }
}

fn text(v: &Value, key: &str, method: &'static str) -> Result<String, RpcError> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| malformed(method, format!("`{key}` is not a string")))
}

fn uint(v: &Value, key: &str, method: &'static str) -> Result<u64, RpcError> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| malformed(method, format!("`{key}` is not an integer")))
}

fn amount(v: &Value, key: &str, method: &'static str) -> Result<Amount, RpcError> {
    let raw = text(v, key, method)?;
    parse_amount(&raw).ok_or_else(|| malformed(method, format!("`{key}` is not a U256: {raw}")))
}

fn address(text: &str, method: &'static str) -> Result<[u8; 32], RpcError> {
    crate::numbers::parse_address(text)
        .ok_or_else(|| malformed(method, format!("address `{text}` is not 32 bytes")))
}

/// Decodes a `get_supply` result.
///
/// # Errors
///
/// Malformed when a field is missing or not a U256.
pub fn decode_supply(v: &Value) -> Result<SupplyFacts, RpcError> {
    Ok(SupplyFacts {
        total: amount(v, "total", "get_supply")?,
        shielded: amount(v, "shielded", "get_supply")?,
        burned: amount(v, "burned", "get_supply")?,
    })
}

/// Decodes a `get_balance_changes` result.
///
/// # Errors
///
/// Malformed when a change is missing a field or names a bad address.
pub fn decode_changes(v: &Value) -> Result<Vec<Change>, RpcError> {
    const M: &str = "get_balance_changes";
    let list = v
        .get("changes")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed(M, "`changes` is not an array"))?;
    list.iter()
        .map(|c| {
            Ok(Change {
                address: address(&text(c, "address", M)?, M)?,
                before: amount(c, "before", M)?,
                after: amount(c, "after", M)?,
            })
        })
        .collect()
}

/// Decodes a `get_block_by_height` result into its transaction outputs.
///
/// # Errors
///
/// Malformed when a transaction or output is missing a field.
pub fn decode_outputs(v: &Value) -> Result<Vec<Output>, RpcError> {
    const M: &str = "get_block_by_height";
    let txs = v
        .get("transactions")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed(M, "`transactions` is not an array"))?;
    let mut out = Vec::new();
    for tx in txs {
        let outputs = tx
            .get("outputs")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed(M, "`outputs` is not an array"))?;
        for o in outputs {
            out.push(Output {
                recipient: address(&text(o, "recipient", M)?, M)?,
                amount: amount(o, "amount", M)?,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn supply_decodes_the_node_shape() {
        let v = json!({"total": "1005000", "circulating": "1005000",
                       "shielded": "0", "burned": "4"});
        let s = decode_supply(&v).unwrap();
        assert_eq!(s.total.to_string(), "1005000");
        assert_eq!(s.burned.to_string(), "4");
    }

    #[test]
    fn a_negative_amount_in_the_wire_is_malformed_not_zero() {
        let v = json!({"total": "-5", "shielded": "0", "burned": "0"});
        assert!(matches!(decode_supply(&v), Err(RpcError::Malformed { .. })));
    }

    #[test]
    fn changes_and_outputs_decode() {
        let addr = hex::encode([7u8; 32]);
        let changes = json!({"height": 3, "block_id": "00", "changes":
            [{"address": addr, "before": "10", "after": "7"}]});
        let c = decode_changes(&changes).unwrap();
        assert_eq!(c[0].address, [7u8; 32]);
        assert_eq!(c[0].before.to_string(), "10");

        let block = json!({"height": 3, "transactions":
            [{"outputs": [{"recipient": hex::encode([1u8; 32]), "amount": "9"}]}]});
        let o = decode_outputs(&block).unwrap();
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].amount.to_string(), "9");
    }

    #[test]
    fn a_missing_changes_list_is_malformed() {
        assert!(matches!(
            decode_changes(&json!({"height": 3})),
            Err(RpcError::Malformed { .. })
        ));
    }
}
