//! [`SwapChain`] over a node's JSON-RPC.

use std::sync::Arc;

use async_trait::async_trait;
use jsonrpsee::core::client::{ClientT, Error as ClientError};
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use tokio::sync::OnceCell;

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
    /// Fetched once and shared by every clone: the genesis of a chain does not
    /// change under a running watcher, and signing asks for it per transaction.
    genesis: Arc<OnceCell<ChainTag>>,
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
            genesis: Arc::new(OnceCell::new()),
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
        // Still u64 here (ADR-046 Priority 2). Refused above u64::MAX, not truncated.
        let balance = u64::try_from(account.balance).map_err(|_| {
            WatcherError::Refused("balance exceeds u64; watcher not yet widened (ADR-046)".into())
        })?;
        Ok(Some((balance, account.nonce)))
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
            base_fee: primitive_types::U256::from_dec_str(&info.base_fee).map_err(|_| {
                WatcherError::Rpc(format!("{} get_fee_info: bad base_fee", self.url))
            })?,
            collector,
        }))
    }

    async fn chain_tag(&self) -> Result<ChainTag> {
        // A failed fetch is not stored, so the next call asks again.
        self.genesis
            .get_or_try_init(|| async {
                let info: ChainInfo = self
                    .client
                    .request("get_chain_info", rpc_params![])
                    .await
                    .map_err(|e| self.failure("get_chain_info", e))?;
                info.chain_tag()
                    .map_err(|e| WatcherError::Rpc(format!("{} chain_tag: {e}", self.url)))
            })
            .await
            .copied()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// The request body of one HTTP/1.1 request: the bytes after the headers,
    /// up to `Content-Length`.
    fn read_body(stream: &mut TcpStream) -> Vec<u8> {
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
    /// how many requests it served.
    fn genesis_node(genesis: String) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let served = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&served);
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
    async fn the_genesis_is_fetched_once_per_chain() {
        let (url, served) = genesis_node("ab".repeat(32));
        let chain = RpcChain::connect(&url).unwrap();

        let first = chain.chain_tag().await.unwrap();
        let second = chain.chain_tag().await.unwrap();
        let clone = chain.clone().chain_tag().await.unwrap();

        // The binding is still the node's genesis, not a placeholder.
        assert_eq!(first, ChainTag::from_genesis([0xab; 32]));
        assert_eq!(first, second);
        assert_eq!(first, clone);
        assert_eq!(served.load(Ordering::SeqCst), 1);
    }
}
