//! [`SwapChain`] over a node's JSON-RPC.

use async_trait::async_trait;
use jsonrpsee::core::client::{ClientT, Error as ClientError};
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;

use custom_l1_node::core::ChainTag;
use custom_l1_node::rpc::{AccountInfo, ChainInfo, FeeInfo, HtlcLockInfo, SubmitTransactionResult};
use maya_htlc_lattice::Address;

use crate::chain::{Fees, LockView, SwapChain};
use crate::error::{Result, WatcherError};
use crate::swap::LockId;

/// A node reached over HTTP JSON-RPC.
#[derive(Debug, Clone)]
pub struct RpcChain {
    client: HttpClient,
    url: String,
}

impl RpcChain {
    /// Connects to a node's JSON-RPC endpoint.
    ///
    /// # Errors
    ///
    /// [`WatcherError::Rpc`] if the URL is unusable.
    pub fn connect(url: &str) -> Result<Self> {
        let client = HttpClientBuilder::default()
            .build(url)
            .map_err(|e| WatcherError::Rpc(format!("connecting to {url}: {e}")))?;
        Ok(Self {
            client,
            url: url.to_owned(),
        })
    }

    /// A call failure, split so a node that answered "no" is not retried as
    /// though it were unreachable.
    fn failure(&self, method: &str, error: ClientError) -> WatcherError {
        match error {
            ClientError::Call(refusal) => {
                WatcherError::Refused(format!("{} {method}: {}", self.url, refusal.message()))
            }
            other => WatcherError::Rpc(format!("{} {method}: {other}", self.url)),
        }
    }
}

#[async_trait]
impl SwapChain for RpcChain {
    async fn tip_height(&self) -> Result<u64> {
        self.client
            .request("get_tip_height", rpc_params![])
            .await
            .map_err(|e| self.failure("get_tip_height", e))
    }

    async fn lock(&self, id: &LockId) -> Result<Option<LockView>> {
        let info: Option<HtlcLockInfo> = self
            .client
            .request("htlc_get_lock", rpc_params![hex::encode(id)])
            .await
            .map_err(|e| self.failure("htlc_get_lock", e))?;
        info.as_ref().map(LockView::from_info).transpose()
    }

    async fn next_nonce(&self, address: &Address) -> Result<u64> {
        let account: AccountInfo = self
            .client
            .request("get_balance", rpc_params![hex::encode(address)])
            .await
            .map_err(|e| self.failure("get_balance", e))?;
        Ok(account.nonce)
    }

    async fn broadcast(&self, raw: &[u8]) -> Result<()> {
        // `accepted: false` means already pooled, which is what a rebroadcast
        // expects.
        let _: SubmitTransactionResult = self
            .client
            .request("send_raw_transaction", rpc_params![hex::encode(raw)])
            .await
            .map_err(|e| self.failure("send_raw_transaction", e))?;
        Ok(())
    }

    async fn account(&self, address: &Address) -> Result<Option<(u64, u64)>> {
        let account: AccountInfo = self
            .client
            .request("get_balance", rpc_params![hex::encode(address)])
            .await
            .map_err(|e| self.failure("get_balance", e))?;
        Ok(Some((account.balance, account.nonce)))
    }

    async fn fees(&self) -> Result<Option<Fees>> {
        let info: FeeInfo = self
            .client
            .request("get_fee_info", rpc_params![])
            .await
            .map_err(|e| self.failure("get_fee_info", e))?;
        if !info.active {
            return Ok(None);
        }
        let collector = hex::decode(&info.collector)
            .ok()
            .and_then(|b| <[u8; 32]>::try_from(b).ok())
            .ok_or_else(|| {
                WatcherError::Rpc(format!("{} get_fee_info: bad collector", self.url))
            })?;
        Ok(Some(Fees {
            base_fee: info.base_fee,
            collector,
        }))
    }

    async fn chain_tag(&self) -> Result<ChainTag> {
        let info: ChainInfo = self
            .client
            .request("get_chain_info", rpc_params![])
            .await
            .map_err(|e| self.failure("get_chain_info", e))?;
        info.chain_tag()
            .map_err(|e| WatcherError::Rpc(format!("{} chain_tag: {e}", self.url)))
    }
}
