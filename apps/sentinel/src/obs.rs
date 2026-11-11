//! What one poll of one node sees, read through the node's own RPC methods.
//!
//! Read-only by construction: the only trait the sentinel has to a node is
//! [`Transport`], and every method it names is a getter. Nothing here signs,
//! submits or mutates.

use std::future::Future;

use primitive_types::U256;
use serde_json::Value;
use thiserror::Error;

/// Anything that answers a JSON-RPC call. The real one is the node's HTTP
/// client; tests script the answers.
pub trait Transport: Send + Sync {
    /// Calls `method` with positional `u64` arguments.
    fn call(
        &self,
        method: &'static str,
        args: Vec<u64>,
    ) -> impl Future<Output = Result<Value, String>> + Send;
}

/// Why a node could not be observed. Only the required calls produce these;
/// optional ones degrade to `None` in [`NodeObs`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ObserveError {
    /// The call failed (unreachable, timed out, or an RPC error).
    #[error("{method} failed: {reason}")]
    Call {
        /// The RPC method that failed.
        method: &'static str,
        /// Transport or RPC error text.
        reason: String,
    },
    /// The call answered, but without the field the sentinel reads.
    #[error("{method} answered without `{field}`")]
    Shape {
        /// The RPC method that answered.
        method: &'static str,
        /// The missing field.
        field: &'static str,
    },
}

/// Supply as the node reports it (`get_supply`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Supply {
    /// Every unit that exists.
    pub total: U256,
    /// Units that can move.
    pub circulating: U256,
    /// Units burned to the fee sink. Only grows.
    pub burned: U256,
    /// Units still in the mining reserve. Zero from a node that predates the
    /// field, whose `burned` already includes them.
    pub unissued: U256,
}

impl Supply {
    /// Whether every unit sits in exactly one bucket. An overflow fails it.
    #[must_use]
    pub fn adds_up(&self) -> bool {
        self.circulating
            .checked_add(self.burned)
            .and_then(|sum| sum.checked_add(self.unissued))
            == Some(self.total)
    }
}

/// One successful observation of a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeObs {
    /// Tip height the node reported.
    pub height: u64,
    /// Hex header id of the block at `height`.
    pub tip_id: String,
    /// Transactions in that block. Zero means an empty block.
    pub tip_txs: usize,
    /// Connected peers, when the node serves `get_peers`.
    pub peers: Option<usize>,
    /// Supply, when this poll asked for it (it scans every account).
    pub supply: Option<Supply>,
    /// Transactions waiting in the mempool, when the node exposes that.
    pub pending: Option<u64>,
}

/// One node's answer in one poll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// The name the operator gave the node (`--node NAME=URL`).
    pub name: String,
    /// The observation, or why there is none.
    pub state: Result<NodeObs, String>,
}

/// Reads one node. `with_supply` gates the account scan.
///
/// # Errors
///
/// [`ObserveError::Call`] when `get_chain_info` or the tip block cannot be
/// read, and [`ObserveError::Shape`] when they answer without the fields the
/// sentinel needs. Peers, supply and mempool are optional and never fail the
/// observation.
pub async fn observe<T: Transport>(t: &T, with_supply: bool) -> Result<NodeObs, ObserveError> {
    let info = t
        .call("get_chain_info", vec![])
        .await
        .map_err(|reason| ObserveError::Call {
            method: "get_chain_info",
            reason,
        })?;
    let height = info
        .get("height")
        .and_then(Value::as_u64)
        .ok_or(ObserveError::Shape {
            method: "get_chain_info",
            field: "height",
        })?;

    let block = t
        .call("get_block_by_height", vec![height])
        .await
        .map_err(|reason| ObserveError::Call {
            method: "get_block_by_height",
            reason,
        })?;
    let tip_id = block
        .pointer("/header/id")
        .and_then(Value::as_str)
        .ok_or(ObserveError::Shape {
            method: "get_block_by_height",
            field: "header.id",
        })?
        .to_owned();
    let tip_txs = block
        .get("transactions")
        .and_then(Value::as_array)
        .map(Vec::len)
        .ok_or(ObserveError::Shape {
            method: "get_block_by_height",
            field: "transactions",
        })?;

    let supply = if with_supply {
        fetch_supply(t).await
    } else {
        None
    };
    let peers = t
        .call("get_peers", vec![])
        .await
        .ok()
        .and_then(|v| Some(v.get("peers")?.as_array()?.len()));
    let pending = t
        .call("get_mempool_info", vec![])
        .await
        .ok()
        .and_then(|v| v.get("pending")?.as_u64());

    Ok(NodeObs {
        height,
        tip_id,
        tip_txs,
        peers,
        supply,
        pending,
    })
}

/// `get_supply` amounts are base-10 strings (`amount_serde`).
async fn fetch_supply<T: Transport>(t: &T) -> Option<Supply> {
    let v = t.call("get_supply", vec![]).await.ok()?;
    let amount = |key: &str| U256::from_dec_str(v.get(key)?.as_str()?).ok();
    Some(Supply {
        total: amount("total")?,
        circulating: amount("circulating")?,
        burned: amount("burned")?,
        unissued: match v.get("unissued") {
            None => U256::zero(),
            Some(_) => amount("unissued")?,
        },
    })
}

/// Polls every node once, in the order given.
pub async fn gather<T: Transport>(nodes: &[(String, T)], with_supply: bool) -> Vec<Sample> {
    let mut out = Vec::with_capacity(nodes.len());
    for (name, transport) in nodes {
        let state = observe(transport, with_supply)
            .await
            .map_err(|e| e.to_string());
        out.push(Sample {
            name: name.clone(),
            state,
        });
    }
    out
}
