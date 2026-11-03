//! [`SwapChain`] over a node's JSON-RPC.

use std::sync::{Arc, Mutex, PoisonError};

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
    /// The genesis this watcher signs with, fetched on first use and shared by
    /// every clone. Cleared when a rejected submission finds the node now
    /// reports a different genesis (a re-genesis), so the next signature uses
    /// the live one.
    genesis: Arc<Mutex<Option<ChainTag>>>,
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
            genesis: Arc::new(Mutex::new(None)),
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

    fn cached_genesis(&self) -> Option<ChainTag> {
        *self.genesis.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn store_genesis(&self, tag: Option<ChainTag>) {
        *self.genesis.lock().unwrap_or_else(PoisonError::into_inner) = tag;
    }

    /// The genesis the node reports now. Never cached.
    async fn live_genesis(&self) -> Result<ChainTag> {
        let info: ChainInfo = self
            .client
            .request("get_chain_info", rpc_params![])
            .await
            .map_err(|e| self.failure("get_chain_info", e))?;
        info.chain_tag()
            .map_err(|e| WatcherError::Rpc(format!("{} chain_tag: {e}", self.url)))
    }

    /// Called on a rejected submission. A node that re-genesised rejects every
    /// signature made under the old tag, so when the live genesis differs from
    /// the cached one this returns [`WatcherError::GenesisChanged`] and clears
    /// the cache. Otherwise, or if the node cannot be asked, the original error
    /// stands.
    async fn rejection(&self, error: WatcherError) -> WatcherError {
        let Some(cached) = self.cached_genesis() else {
            return error;
        };
        match self.live_genesis().await {
            Ok(live) if live != cached => {
                self.store_genesis(None);
                let changed = WatcherError::GenesisChanged {
                    cached: hex::encode(cached.0),
                    live: hex::encode(live.0),
                };
                eprintln!("{changed}");
                changed
            }
            _ => error,
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
        let submitted: Result<SubmitTransactionResult> = self
            .client
            .request("send_raw_transaction", rpc_params![hex::encode(raw)])
            .await
            .map_err(|e| self.failure("send_raw_transaction", e));
        match submitted {
            Ok(_) => Ok(()),
            Err(error) => Err(self.rejection(error).await),
        }
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
        if let Some(tag) = self.cached_genesis() {
            return Ok(tag);
        }
        // A failed fetch is not stored, so the next call asks again.
        let tag = self.live_genesis().await?;
        self.store_genesis(Some(tag));
        Ok(tag)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Mutex;
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

    /// A loopback node. It answers `get_chain_info` with whatever genesis is
    /// in `genesis` at that moment, so a test can re-genesis it mid-run, and it
    /// rejects every `send_raw_transaction`. Returns its URL and a count of the
    /// requests it served.
    fn node(genesis: Arc<Mutex<String>>) -> (String, Arc<AtomicUsize>) {
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
                let reply = if request["method"] == "get_chain_info" {
                    let genesis = genesis.lock().unwrap().clone();
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "result": {"genesis": genesis, "chain_id": null, "height": 0},
                    })
                } else {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "error": {"code": -32000, "message": "transaction rejected"},
                    })
                }
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

    fn shared(hex_genesis: &str) -> Arc<Mutex<String>> {
        Arc::new(Mutex::new(hex_genesis.to_owned()))
    }

    #[tokio::test]
    async fn the_genesis_is_fetched_once_per_chain() {
        let (url, served) = node(shared(&"ab".repeat(32)));
        let chain = RpcChain::connect(&url).unwrap();

        let first = chain.chain_tag().await.unwrap();
        let second = chain.chain_tag().await.unwrap();
        let clone = chain.clone().chain_tag().await.unwrap();

        // The binding is still the node's genesis, not a placeholder.
        assert_eq!(first, ChainTag::from_genesis([0xab; 32]));
        assert_eq!(first, second);
        assert_eq!(first, clone);
        // Three signatures' worth of lookups, one request.
        assert_eq!(served.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_re_genesised_node_is_reported_and_the_next_signature_uses_the_live_tag() {
        let genesis = shared(&"ab".repeat(32));
        let (url, _) = node(Arc::clone(&genesis));
        let chain = RpcChain::connect(&url).unwrap();
        assert_eq!(
            chain.chain_tag().await.unwrap(),
            ChainTag::from_genesis([0xab; 32])
        );

        // The node re-geneses; the watcher's signed transaction is rejected.
        *genesis.lock().unwrap() = "cd".repeat(32);
        let error = chain.broadcast(&[1, 2, 3]).await.unwrap_err();
        assert!(
            matches!(
                &error,
                WatcherError::GenesisChanged { cached, live }
                    if *cached == "ab".repeat(32) && *live == "cd".repeat(32)
            ),
            "expected GenesisChanged, got {error:?}"
        );

        // The cache was cleared, so the next attempt signs with the live tag.
        assert_eq!(
            chain.chain_tag().await.unwrap(),
            ChainTag::from_genesis([0xcd; 32])
        );
    }

    #[tokio::test]
    async fn a_rejection_with_an_unchanged_genesis_stays_a_refusal() {
        let (url, _) = node(shared(&"ab".repeat(32)));
        let chain = RpcChain::connect(&url).unwrap();
        chain.chain_tag().await.unwrap();

        let error = chain.broadcast(&[1, 2, 3]).await.unwrap_err();
        assert!(
            matches!(error, WatcherError::Refused(_)),
            "expected the node's refusal, got {error:?}"
        );
        assert_eq!(
            chain.chain_tag().await.unwrap(),
            ChainTag::from_genesis([0xab; 32])
        );
    }
}
