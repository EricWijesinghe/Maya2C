//! Live values from the node's JSON-RPC: what history cannot answer.
//!
//! Behind a trait so the API tests can stand in a node that counts its calls,
//! fails on demand, or answers slowly. The production implementation is
//! [`RpcNode`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use custom_l1_node::rpc::types::CheckpointInfo;
use custom_l1_node::rpc::validators::ValidatorsInfo;
use custom_l1_node::rpc::{AccountInfo, ChainInfo, SupplyReport};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use serde::Deserialize;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

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
    /// Hex id of the node's block at `height` (`get_block_by_height`), so a
    /// response is only marked immutable for the block the chain kept.
    async fn block_id(&self, height: u64) -> Result<String>;
}

/// How long one node call may take. jsonrpsee's default is 60 s, which lets a
/// stalled node hold every request (and every node slot) for a minute.
/// shortcut: one figure for every method, `get_supply` included; give the scan
/// its own if a large state ever needs longer.
pub const NODE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a call waits for a node slot before it is answered
/// `node_unavailable`.
pub const SLOT_WAIT: Duration = Duration::from_secs(2);

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
            .request_timeout(NODE_TIMEOUT)
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

    async fn block_id(&self, height: u64) -> Result<String> {
        // Only the header id is read; serde skips the rest of the block.
        #[derive(Deserialize)]
        struct Header {
            id: String,
        }
        #[derive(Deserialize)]
        struct Block {
            header: Header,
        }
        let block: Block = self
            .call("get_block_by_height", rpc_params![height])
            .await?;
        Ok(block.header.id)
    }
}

/// A [`NodeSource`] that holds one of a shared set of slots per call.
///
/// The explorer reaches the node from one address, and the node limits calls
/// per address, so the v1 API and the legacy pages draw on the same slots
/// (`AppState::node_calls`). A call waits up to [`SLOT_WAIT`] for one.
pub struct Limited {
    inner: Arc<dyn NodeSource>,
    slots: Arc<Semaphore>,
}

impl Limited {
    /// Wraps `inner`, drawing slots from `slots`.
    #[must_use]
    pub fn new(inner: Arc<dyn NodeSource>, slots: Arc<Semaphore>) -> Self {
        Self { inner, slots }
    }

    async fn slot(&self) -> Result<OwnedSemaphorePermit> {
        let busy = || ExplorerError::NodeRpc("no node slot free".to_string());
        tokio::time::timeout(SLOT_WAIT, Arc::clone(&self.slots).acquire_owned())
            .await
            .map_err(|_| busy())?
            .map_err(|_| busy())
    }
}

#[async_trait]
impl NodeSource for Limited {
    async fn chain(&self) -> Result<ChainInfo> {
        let _slot = self.slot().await?;
        self.inner.chain().await
    }
    async fn checkpoint(&self) -> Result<Option<CheckpointInfo>> {
        let _slot = self.slot().await?;
        self.inner.checkpoint().await
    }
    async fn account(&self, address_hex: &str) -> Result<AccountInfo> {
        let _slot = self.slot().await?;
        self.inner.account(address_hex).await
    }
    async fn validators(&self) -> Result<Option<ValidatorsInfo>> {
        let _slot = self.slot().await?;
        self.inner.validators().await
    }
    async fn supply(&self) -> Result<SupplyReport> {
        let _slot = self.slot().await?;
        self.inner.supply().await
    }
    async fn mempool(&self) -> Result<Mempool> {
        let _slot = self.slot().await?;
        self.inner.mempool().await
    }
    async fn block_id(&self, height: u64) -> Result<String> {
        let _slot = self.slot().await?;
        self.inner.block_id(height).await
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
    async fn block_id(&self, _height: u64) -> Result<String> {
        Err(unconfigured())
    }
}

fn unconfigured() -> ExplorerError {
    ExplorerError::NodeRpc("no node endpoint configured".to_string())
}
