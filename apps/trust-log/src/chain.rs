//! What this crate reads from a node: the tip height and a block by height,
//! over the node's public JSON-RPC (`get_chain_info`, `get_block_by_height`).

use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use serde_json::Value;

use crate::{Error, Result};

/// Per-call deadline.
const REQUEST_TIMEOUT_SECS: u64 = 10;

/// A read-only view of a chain.
pub trait Chain {
    /// The current tip height.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the source cannot answer.
    fn tip_height(&self) -> Result<u64>;

    /// The block at `height`, as `get_block_by_height` returns it.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when the source cannot answer.
    fn block(&self, height: u64) -> Result<Value>;
}

/// A node's JSON-RPC endpoint.
pub struct Rpc {
    client: HttpClient,
    runtime: tokio::runtime::Runtime,
}

impl Rpc {
    /// A client for `url`.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] for a URL the client cannot use.
    pub fn new(url: &str) -> Result<Self> {
        let client = HttpClientBuilder::default()
            .request_timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build(url)
            .map_err(|e| Error::Io(format!("{url}: {e}")))?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Io(e.to_string()))?;
        Ok(Self { client, runtime })
    }
}

impl Chain for Rpc {
    fn tip_height(&self) -> Result<u64> {
        let info: Value = self
            .runtime
            .block_on(self.client.request("get_chain_info", rpc_params![]))
            .map_err(|e| Error::Io(format!("get_chain_info: {e}")))?;
        info["height"]
            .as_u64()
            .ok_or_else(|| Error::Malformed("get_chain_info has no height".into()))
    }

    fn block(&self, height: u64) -> Result<Value> {
        self.runtime
            .block_on(
                self.client
                    .request("get_block_by_height", rpc_params![height]),
            )
            .map_err(|e| Error::Io(format!("get_block_by_height {height}: {e}")))
    }
}

/// The id of the block at `height`.
///
/// # Errors
///
/// As [`Chain::block`], and [`Error::Malformed`] for a block with no id.
pub fn block_id(chain: &dyn Chain, height: u64) -> Result<String> {
    chain.block(height)?["header"]["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::Malformed(format!("block {height} has no header id")))
}

#[cfg(test)]
pub(crate) mod fake {
    use std::collections::BTreeMap;

    use super::{Chain, Value};
    use crate::{Error, Result};

    /// An in-memory chain for tests.
    #[derive(Default)]
    pub(crate) struct FakeChain {
        pub(crate) tip: u64,
        pub(crate) blocks: BTreeMap<u64, Value>,
    }

    impl Chain for FakeChain {
        fn tip_height(&self) -> Result<u64> {
            Ok(self.tip)
        }
        fn block(&self, height: u64) -> Result<Value> {
            self.blocks
                .get(&height)
                .cloned()
                .ok_or_else(|| Error::Io(format!("no block at height {height}")))
        }
    }
}
