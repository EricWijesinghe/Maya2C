//! Live values from the node's JSON-RPC: what history cannot answer.
//!
//! Behind a trait so the API tests can stand in a node that counts its calls,
//! fails on demand, or answers slowly. The production implementation is
//! [`RpcNode`].

use async_trait::async_trait;
use custom_l1_node::rpc::types::CheckpointInfo;
use custom_l1_node::rpc::validators::ValidatorsInfo;
use custom_l1_node::rpc::{AccountInfo, ChainInfo, SupplyReport};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use serde::Deserialize;

use crate::error::{ExplorerError, Result};

/// The node's mempool occupancy, as `get_mempool_info` reports it.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub struct Mempool {
    /// Transactions waiting.
    pub size: u64,
    /// Their total encoded size in bytes.
    pub bytes: u64,
}

/// Node queries the v1 API makes. Each returns the node's own answer.
#[async_trait]
pub trait NodeSource: Send + Sync {
    /// Chain identity and tip (`get_chain_info`).
    async fn chain(&self) -> Result<ChainInfo>;
    /// The newest finality checkpoint, or `None` when the chain has none.
    async fn checkpoint(&self) -> Result<Option<CheckpointInfo>>;
    /// Balance and next nonce of a hex address (`get_balance`).
    async fn account(&self, address_hex: &str) -> Result<AccountInfo>;
    /// Validator set, or `None` without staking (`get_validators`).
    async fn validators(&self) -> Result<Option<ValidatorsInfo>>;
    /// Supply figures (`get_supply`). A full scan of every account on the node.
    async fn supply(&self) -> Result<SupplyReport>;
    /// Mempool occupancy (`get_mempool_info`).
    async fn mempool(&self) -> Result<Mempool>;
}

/// A node reached over its JSON-RPC port.
pub struct RpcNode {
    client: HttpClient,
}

impl RpcNode {
    /// Builds a client for `url`. No connection is made until a call.
    ///
    /// # Errors
    ///
    /// [`ExplorerError::NodeRpc`] if the URL cannot be used.
    pub fn connect(url: &str) -> Result<Self> {
        let client = HttpClientBuilder::default()
            .build(url)
            .map_err(|e| ExplorerError::NodeRpc(e.to_string()))?;
        Ok(Self { client })
    }

    async fn call<T>(&self, method: &str, params: jsonrpsee::core::params::ArrayParams) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.client
            .request(method, params)
            .await
            .map_err(|e| ExplorerError::NodeRpc(format!("{method}: {e}")))
    }
}

#[async_trait]
impl NodeSource for RpcNode {
    async fn chain(&self) -> Result<ChainInfo> {
        self.call("get_chain_info", rpc_params![]).await
    }

    async fn checkpoint(&self) -> Result<Option<CheckpointInfo>> {
        self.call("get_checkpoint", rpc_params![]).await
    }

    async fn account(&self, address_hex: &str) -> Result<AccountInfo> {
        self.call("get_balance", rpc_params![address_hex]).await
    }

    async fn validators(&self) -> Result<Option<ValidatorsInfo>> {
        self.call("get_validators", rpc_params![]).await
    }

    async fn supply(&self) -> Result<SupplyReport> {
        self.call("get_supply", rpc_params![]).await
    }

    async fn mempool(&self) -> Result<Mempool> {
        self.call("get_mempool_info", rpc_params![]).await
    }
}

/// Stands in when no node URL is configured: every live call fails, so the
/// API reports `node_unavailable` while history keeps working.
pub struct NoNode;

#[async_trait]
impl NodeSource for NoNode {
    async fn chain(&self) -> Result<ChainInfo> {
        Err(unconfigured())
    }
    async fn checkpoint(&self) -> Result<Option<CheckpointInfo>> {
        Err(unconfigured())
    }
    async fn account(&self, _address_hex: &str) -> Result<AccountInfo> {
        Err(unconfigured())
    }
    async fn validators(&self) -> Result<Option<ValidatorsInfo>> {
        Err(unconfigured())
    }
    async fn supply(&self) -> Result<SupplyReport> {
        Err(unconfigured())
    }
    async fn mempool(&self) -> Result<Mempool> {
        Err(unconfigured())
    }
}

fn unconfigured() -> ExplorerError {
    ExplorerError::NodeRpc("no node endpoint configured".to_string())
}
