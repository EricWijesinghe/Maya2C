//! The node, as this service needs it: three JSON-RPC reads behind a trait so
//! the handlers can be tested against a recorded node.

use async_trait::async_trait;
use jsonrpsee::core::ClientError;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use serde::Deserialize;

/// How long one node call may take.
pub const NODE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// JSON-RPC code the node uses for "no such block / not kept".
const NOT_FOUND_CODE: i32 = -32_001;

/// A block header, as the node reports it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Header {
    /// Hex id.
    pub id: String,
    /// Hex parent id.
    pub prev_hash: String,
    /// Unix seconds.
    pub timestamp: u64,
}

/// A transaction, reduced to its id.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Tx {
    /// Hex id.
    pub txid: String,
}

/// A block, as the node reports it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct NodeBlock {
    /// Height.
    pub height: u64,
    /// Header.
    pub header: Header,
    /// Transactions.
    pub transactions: Vec<Tx>,
}

/// One balance change.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Change {
    /// Hex address.
    pub address: String,
    /// Before the block.
    pub before: u64,
    /// After it.
    pub after: u64,
}

/// A block's balance changes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Changes {
    /// Hex block id they belong to.
    pub block_id: String,
    /// The changes.
    pub changes: Vec<Change>,
}

/// An account read at the tip.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AtTip {
    /// Balance.
    pub balance: u64,
    /// Tip height.
    pub height: u64,
    /// Hex tip id.
    pub block_id: String,
}

/// Why a node read failed.
#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    /// The node has no such block, or no longer keeps what was asked.
    #[error("not found: {0}")]
    NotFound(String),
    /// Anything else: transport, timeout, decoding.
    #[error("node unavailable: {0}")]
    Unavailable(String),
}

/// What this service reads from a node.
#[async_trait]
pub trait Node: Send + Sync + 'static {
    /// The block at `height`.
    async fn block(&self, height: u64) -> Result<NodeBlock, NodeError>;
    /// Every balance the block at `height` moved; `NotFound` for genesis.
    async fn balance_changes(&self, height: u64) -> Result<Changes, NodeError>;
    /// An account's balance at the current tip, with that tip.
    async fn account_at_tip(&self, address: &str) -> Result<AtTip, NodeError>;
    /// An account's balance after the block at `height`.
    async fn balance_at(&self, address: &str, height: u64) -> Result<AtTip, NodeError>;
}

/// A node over JSON-RPC.
pub struct RpcNode {
    client: HttpClient,
}

impl RpcNode {
    /// Connects.
    ///
    /// # Errors
    ///
    /// [`NodeError::Unavailable`] for an unusable URL.
    pub fn connect(url: &str) -> Result<Self, NodeError> {
        let client = HttpClientBuilder::default()
            .request_timeout(NODE_TIMEOUT)
            .build(url)
            .map_err(|e| NodeError::Unavailable(e.to_string()))?;
        Ok(Self { client })
    }

    async fn call<R: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: jsonrpsee::core::params::ArrayParams,
    ) -> Result<R, NodeError> {
        self.client
            .request(method, params)
            .await
            .map_err(|e| match &e {
                ClientError::Call(call) if call.code() == NOT_FOUND_CODE => {
                    NodeError::NotFound(call.message().to_string())
                }
                _ => NodeError::Unavailable(format!("{method}: {e}")),
            })
    }
}

#[async_trait]
impl Node for RpcNode {
    async fn block(&self, height: u64) -> Result<NodeBlock, NodeError> {
        self.call("get_block_by_height", rpc_params![height]).await
    }

    async fn balance_changes(&self, height: u64) -> Result<Changes, NodeError> {
        self.call("get_balance_changes", rpc_params![height]).await
    }

    async fn account_at_tip(&self, address: &str) -> Result<AtTip, NodeError> {
        self.call("get_account_at_tip", rpc_params![address]).await
    }

    async fn balance_at(&self, address: &str, height: u64) -> Result<AtTip, NodeError> {
        self.call("get_balance_at_height", rpc_params![address, height])
            .await
    }
}
