//! Thin JSON-RPC client for the node.

use anyhow::{Context, Result};
use custom_l1_node::rpc::{AccountInfo, BlockInfo, SubmitTransactionResult};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;

/// Connected client for a node's RPC endpoint.
pub struct NodeClient {
    inner: HttpClient,
    url: String,
}

impl NodeClient {
    /// Connects to `url`.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL is malformed.
    pub fn connect(url: &str) -> Result<Self> {
        let inner = HttpClientBuilder::default()
            .build(url)
            .with_context(|| format!("building an RPC client for {url}"))?;
        Ok(Self {
            inner,
            url: url.to_string(),
        })
    }

    /// The endpoint this client talks to.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Fetches an account's balance and nonce.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the node rejects the address.
    pub async fn get_balance(&self, address_hex: &str) -> Result<AccountInfo> {
        self.inner
            .request("get_balance", rpc_params![address_hex])
            .await
            .with_context(|| format!("get_balance({address_hex})"))
    }

    /// Broadcasts a hex-encoded signed transaction.
    ///
    /// # Errors
    ///
    /// Returns an error if the node rejects the transaction.
    pub async fn send_raw_transaction(&self, tx_hex: &str) -> Result<SubmitTransactionResult> {
        self.inner
            .request("send_raw_transaction", rpc_params![tx_hex])
            .await
            .context("send_raw_transaction")
    }

    /// Fetches the block at `height` on the active chain.
    ///
    /// # Errors
    ///
    /// Returns an error if no block exists at that height.
    pub async fn get_block_by_height(&self, height: u64) -> Result<BlockInfo> {
        self.inner
            .request("get_block_by_height", rpc_params![height])
            .await
            .with_context(|| format!("get_block_by_height({height})"))
    }
}
