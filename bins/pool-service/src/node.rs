//! The node, as the pool sees it.
//!
//! A thin wrapper over the five JSON-RPC methods the pool actually uses. It is
//! its own type rather than `l1_wallet::client::NodeClient` because that client
//! covers the wallet's three methods and reports failures as `anyhow` — the
//! pool needs `get_mining_candidate` and `submit_block` as well, and needs a
//! typed [`PoolError::NodeRpc`] so a retry loop can tell an unreachable chain
//! from an unusable ledger.
//!
//! ## Every call here can fail, and the pool must keep serving when it does
//!
//! An unreachable node means work goes stale and blocks cannot be submitted. It
//! does **not** mean accepted shares are lost: those are already in the ledger,
//! and the credits they carry are safe. The error variant this module returns
//! is the one that says so.

use std::sync::Arc;

use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use tokio::sync::OnceCell;

use custom_l1_node::core::ChainTag;
use custom_l1_node::rpc::{
    AccountInfo, BlockInfo, ChainInfo, MiningCandidate, SubmitBlockResult, SubmitTransactionResult,
};

use crate::error::{PoolError, Result};

/// Outcome strings `submit_block` reports for a block that reached the chain.
///
/// `side_branch` counts: the block is valid and on a fork the chain has not
/// chosen. Treating it as a failure would discard credits for real work, and
/// the confirmation watcher is the thing that decides whether it stays.
const ACCEPTED_OUTCOMES: &[&str] = &["extended", "reorganized", "side_branch", "duplicate"];

/// A JSON-RPC connection to a node.
#[derive(Debug, Clone)]
pub struct NodeClient {
    /// The underlying HTTP client.
    client: HttpClient,
    /// Endpoint, for error messages.
    url: String,
    /// Fetched once and shared by every clone: the genesis does not change
    /// under a running pool, and payout signing asks for it on every batch.
    genesis: Arc<OnceCell<ChainTag>>,
}

impl NodeClient {
    /// Connects to a node's JSON-RPC endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::NodeRpc`] if the URL is unusable.
    pub fn connect(url: &str) -> Result<Self> {
        let client = HttpClientBuilder::default()
            .build(url)
            .map_err(|e| PoolError::NodeRpc(format!("connecting to {url}: {e}")))?;
        Ok(Self {
            client,
            url: url.to_string(),
            genesis: Arc::new(OnceCell::new()),
        })
    }

    /// The endpoint this client talks to.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Fetches a fresh work template.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::NodeRpc`] if the node is unreachable or refuses.
    pub async fn mining_candidate(&self) -> Result<MiningCandidate> {
        self.client
            .request("get_mining_candidate", rpc_params![])
            .await
            .map_err(|e| PoolError::NodeRpc(format!("get_mining_candidate: {e}")))
    }

    /// Submits a solved block.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::NodeRpc`] on a transport failure, or
    /// [`PoolError::Chain`] when the node accepted the request and rejected the
    /// block. The two are separated because only the first is worth retrying:
    /// a block the chain refused will be refused again.
    pub async fn submit_block(&self, block: &custom_l1_node::core::Block) -> Result<String> {
        let raw = hex::encode(block.to_bytes());
        let result: SubmitBlockResult = self
            .client
            .request("submit_block", rpc_params![raw])
            .await
            .map_err(|e| PoolError::NodeRpc(format!("submit_block: {e}")))?;

        if !ACCEPTED_OUTCOMES.contains(&result.outcome.as_str()) {
            return Err(PoolError::Chain(format!(
                "the chain refused a block the pool verified: {}",
                result.outcome
            )));
        }

        Ok(result.outcome)
    }

    /// Broadcasts a signed transaction.
    ///
    /// # Errors
    ///
    /// As [`NodeClient::submit_block`]: transport failures are
    /// [`PoolError::NodeRpc`], a refusal by the mempool is [`PoolError::Chain`].
    pub async fn send_raw_transaction(&self, raw: &[u8]) -> Result<String> {
        let result: SubmitTransactionResult = self
            .client
            .request("send_raw_transaction", rpc_params![hex::encode(raw)])
            .await
            .map_err(|e| PoolError::NodeRpc(format!("send_raw_transaction: {e}")))?;

        if !result.accepted {
            return Err(PoolError::Chain(format!(
                "the mempool refused payout transaction {}",
                result.txid
            )));
        }

        Ok(result.txid)
    }

    /// Reads an account's balance and nonce.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::NodeRpc`] if the node is unreachable or refuses.
    pub async fn account_info(&self, address: &[u8; 32]) -> Result<AccountInfo> {
        self.client
            .request("get_balance", rpc_params![hex::encode(address)])
            .await
            .map_err(|e| PoolError::NodeRpc(format!("get_balance: {e}")))
    }

    /// Reads a block by height.
    ///
    /// Returns `Ok(None)` for a height the chain has not reached, which is an
    /// ordinary condition for the confirmation watcher and not an error.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::NodeRpc`] on a transport failure.
    pub async fn block_by_height(&self, height: u64) -> Result<Option<BlockInfo>> {
        match self
            .client
            .request::<BlockInfo, _>("get_block_by_height", rpc_params![height])
            .await
        {
            Ok(block) => Ok(Some(block)),
            // The node answers a height past the tip with an application error
            // rather than a null, so an absent block and an unreachable node
            // are the same shape on the wire. Distinguishing them by string
            // would be fragile; treating a failed lookup as "not yet" is
            // correct for the one caller, which polls until it appears.
            Err(_) => Ok(None),
        }
    }

    /// Fetches the chain's genesis block id (for signing transactions).
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::NodeRpc`] on a transport or parsing failure.
    pub async fn get_chain_tag(&self) -> Result<ChainTag> {
        // A failed fetch is not stored, so the next call asks again.
        self.genesis
            .get_or_try_init(|| async {
                let info: ChainInfo = self
                    .client
                    .request("get_chain_info", rpc_params![])
                    .await
                    .map_err(|e| PoolError::NodeRpc(format!("get_chain_info: {e}")))?;
                info.chain_tag()
                    .map_err(|e| PoolError::NodeRpc(format!("chain_tag: {e}")))
            })
            .await
            .copied()
    }
}

/// The chain, as the payout engine needs it.
///
/// `chain_height` is derived from the mining candidate rather than from a
/// dedicated call, because the node exposes no `get_height`: the candidate is
/// the header for the *next* block, so the active tip is one below it. That is
/// an exact relationship (`src/rpc/server.rs:167`), not an approximation, and
/// the pool is already polling the candidate anyway.
#[async_trait::async_trait]
impl crate::payout::ChainView for NodeClient {
    async fn chain_height(&self) -> Result<u64> {
        let candidate = self.mining_candidate().await?;
        Ok(candidate.height.saturating_sub(1))
    }

    async fn block_id_at(&self, height: u64) -> Result<Option<String>> {
        Ok(self
            .block_by_height(height)
            .await?
            .map(|block| block.header.id))
    }

    async fn account(&self, address: &[u8; 32]) -> Result<(primitive_types::U256, u64)> {
        let info = self.account_info(address).await?;
        Ok((info.balance, info.nonce))
    }

    async fn broadcast(&self, raw: &[u8]) -> Result<String> {
        self.send_raw_transaction(raw).await
    }

    async fn chain_tag(&self) -> Result<ChainTag> {
        self.get_chain_tag().await
    }
}

/// `value`, or `u64::MAX` when it does not fit.
pub(crate) fn saturating_u64(value: primitive_types::U256) -> u64 {
    if value > primitive_types::U256::from(u64::MAX) {
        u64::MAX
    } else {
        value.low_u64()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_side_branch_block_counts_as_accepted() {
        // A valid block on a fork the chain has not chosen is still work the
        // pool must credit; the confirmation watcher decides whether it stays.
        assert!(ACCEPTED_OUTCOMES.contains(&"side_branch"));
        assert!(ACCEPTED_OUTCOMES.contains(&"extended"));
        assert!(ACCEPTED_OUTCOMES.contains(&"duplicate"));
    }

    #[test]
    fn an_unusable_url_fails_at_connect_rather_than_at_first_use() {
        assert!(matches!(
            NodeClient::connect("not a url"),
            Err(PoolError::NodeRpc(_))
        ));
    }

    /// The request body of one HTTP/1.1 request, up to `Content-Length`.
    fn read_body(stream: &mut std::net::TcpStream) -> Vec<u8> {
        use std::io::Read;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = stream.read(&mut chunk).unwrap();
            assert!(n > 0, "client closed before the request was complete");
            buf.extend_from_slice(&chunk[..n]);
            let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&buf[..end]).to_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .map_or(0, |v| v.trim().parse::<usize>().unwrap());
            if buf.len() >= end + 4 + len {
                return buf[end + 4..end + 4 + len].to_vec();
            }
        }
    }

    /// A loopback node that answers `get_chain_info` with `genesis` and counts
    /// the requests it serves.
    fn genesis_node(genesis: String) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::Write;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let served = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = std::sync::Arc::clone(&served);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let request: serde_json::Value =
                    serde_json::from_slice(&read_body(&mut stream)).unwrap();
                counter.fetch_add(1, Ordering::SeqCst);
                let reply = serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "result": {"genesis": genesis, "chain_id": null, "height": 0},
                })
                .to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                )
                .unwrap();
            }
        });
        (url, served)
    }

    #[tokio::test]
    async fn the_genesis_is_fetched_once_per_node_client() {
        use std::sync::atomic::Ordering;
        let (url, served) = genesis_node("ab".repeat(32));
        let node = NodeClient::connect(&url).unwrap();

        let first = node.get_chain_tag().await.unwrap();
        let second = node.get_chain_tag().await.unwrap();
        let clone = node.clone().get_chain_tag().await.unwrap();

        // The binding is still the node's genesis, not a placeholder.
        assert_eq!(first, ChainTag::from_genesis([0xab; 32]));
        assert_eq!(first, second);
        assert_eq!(first, clone);
        assert_eq!(served.load(Ordering::SeqCst), 1);
    }
}
