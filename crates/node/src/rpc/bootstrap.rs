//! Serving and fetching what a pruned node bootstraps from, over JSON-RPC.
//!
//! Server: `get_tip_height`, `get_headers`, `get_snapshot_manifest` and
//! `get_snapshot_chunk`, next to the existing `get_block_by_height`.
//! Client: [`RpcBootstrapSource`], a
//! [`crate::state_pruner::snapshot::BootstrapSource`] over those methods.
//!
//! JSON-RPC rather than a libp2p protocol for now, as `backfill_loop` already
//! does for history. Nothing here is trusted: the bootstrap validates every
//! header, checks every chunk against the manifest, and checks the imported
//! state against the header's root.

use std::sync::{Arc, Mutex};

use jsonrpsee::RpcModule;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use jsonrpsee::types::ErrorObjectOwned;

use crate::core::{Block, BlockHeader};
use crate::error::{NodeError, Result};
use crate::rpc::server::RpcContext;
use crate::rpc::types::BlockInfo;
use crate::state_pruner::snapshot::{BootstrapSource, HEADER_BATCH, SnapshotManifest, Snapshots};

/// JSON-RPC error code for a request the node refused.
const REJECTED: i32 = -32_000;

fn rejected(message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(REJECTED, message.into(), None::<()>)
}

/// Loaded snapshots kept in memory: one per checkpoint the node keeps on disk.
///
/// At least as many as are kept, or a caller alternating between two heights
/// forces a full reopen and re-scan of the state on every request. That was an
/// unauthenticated way to make the node do unbounded work.
const LOADED_SNAPSHOTS: usize = 2;

/// A loaded snapshot: its manifest and every chunk.
type Loaded = (SnapshotManifest, Vec<Vec<u8>>);

/// What a node serves snapshots from, and the snapshots it has loaded.
///
/// The cache matters: a chunk request would otherwise reopen and re-scan the
/// whole checkpoint once per chunk.
pub struct SnapshotService {
    snapshots: Snapshots,
    min_depth: u64,
    /// Most recently used last.
    cache: Mutex<Vec<Loaded>>,
}

impl SnapshotService {
    /// Serves the newest snapshot in `snapshots` at least `min_depth` deep.
    #[must_use]
    pub fn new(snapshots: Snapshots, min_depth: u64) -> Self {
        Self {
            snapshots,
            min_depth,
            cache: Mutex::new(Vec::new()),
        }
    }

    /// The snapshots directory.
    #[must_use]
    pub fn snapshots(&self) -> &Snapshots {
        &self.snapshots
    }

    fn loaded(&self, height: u64) -> Result<Loaded> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(index) = cache.iter().position(|(m, _)| m.height == height) {
            let hit = cache.remove(index);
            cache.push(hit.clone());
            return Ok(hit);
        }
        // Only a checkpoint that exists: a made-up height costs a directory
        // listing, never a state scan.
        if !self.snapshots.heights()?.contains(&height) {
            return Err(NodeError::Network(format!(
                "no snapshot at height {height}"
            )));
        }
        let loaded = self.snapshots.load(height)?;
        if cache.len() == LOADED_SNAPSHOTS {
            cache.remove(0);
        }
        cache.push(loaded.clone());
        Ok(loaded)
    }
}

/// Registers the bootstrap methods on `module`.
///
/// # Errors
///
/// Returns an error if a method name is already registered.
pub fn register(module: &mut RpcModule<RpcContext>) -> std::result::Result<(), ErrorObjectOwned> {
    module
        .register_method("get_tip_height", |_, ctx, _| {
            Ok::<_, ErrorObjectOwned>(ctx.chain().height())
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_headers", |params, ctx, _| {
            let (from, to): (u64, u64) = params.parse().map_err(|e| rejected(e.to_string()))?;
            if to < from || to - from >= HEADER_BATCH {
                return Err(rejected(format!("ask for at most {HEADER_BATCH} headers")));
            }
            let chain = ctx.chain();
            let state = chain.state();
            (from..=to)
                .map(|height| {
                    let id = state
                        .canonical_id(height)
                        .map_err(|e| rejected(e.to_string()))?
                        .ok_or_else(|| rejected(format!("no block at height {height}")))?;
                    let stored = state
                        .stored_header(&id)
                        .map_err(|e| rejected(e.to_string()))?
                        .ok_or_else(|| rejected(format!("no header at height {height}")))?;
                    Ok(hex::encode(stored.header.serialize()))
                })
                .collect::<std::result::Result<Vec<String>, ErrorObjectOwned>>()
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_snapshot_manifest", |_, ctx, _| {
            let service = ctx
                .snapshots
                .as_ref()
                .ok_or_else(|| rejected("this node serves no snapshots"))?;
            let tip = ctx.chain().height();
            let height = service
                .snapshots
                .serveable(tip, service.min_depth)
                .map_err(|e| rejected(e.to_string()))?
                .ok_or_else(|| rejected("no snapshot is deep enough yet"))?;
            let (manifest, _) = service
                .loaded(height)
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(hex::encode(manifest.encode()))
        })
        .map_err(|e| rejected(e.to_string()))?;

    module
        .register_method("get_snapshot_chunk", |params, ctx, _| {
            let (height, index): (u64, usize) =
                params.parse().map_err(|e| rejected(e.to_string()))?;
            let service = ctx
                .snapshots
                .as_ref()
                .ok_or_else(|| rejected("this node serves no snapshots"))?;
            let (_, chunks) = service
                .loaded(height)
                .map_err(|e| rejected(e.to_string()))?;
            let chunk = chunks
                .get(index)
                .ok_or_else(|| rejected(format!("no chunk {index}")))?;
            Ok::<_, ErrorObjectOwned>(hex::encode(chunk))
        })
        .map_err(|e| rejected(e.to_string()))?;

    Ok(())
}

/// A bootstrap source over another node's JSON-RPC endpoint.
///
/// Synchronous, to match [`BootstrapSource`]. Run the bootstrap on a blocking
/// thread (`tokio::task::spawn_blocking`); each call blocks on the runtime
/// handle it was built with.
pub struct RpcBootstrapSource {
    client: HttpClient,
    runtime: tokio::runtime::Handle,
    snapshot_height: Arc<Mutex<Option<u64>>>,
}

impl RpcBootstrapSource {
    /// A source reading from the node at `url`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] for a URL the client cannot use.
    pub fn new(url: &str, runtime: tokio::runtime::Handle) -> Result<Self> {
        Ok(Self {
            client: HttpClientBuilder::default()
                .build(url)
                .map_err(|e| NodeError::Network(format!("{url}: {e}")))?,
            runtime,
            snapshot_height: Arc::default(),
        })
    }

    /// One request, retried with backoff when the transport fails: a public
    /// bootstrap endpoint rate-limits per client (HTTP 429), and a joining
    /// node that gave up on the first refusal would have to start over. A
    /// JSON-RPC error from the peer is an answer, not a failure, and is
    /// returned at once. Blocks this thread while it waits; every caller runs
    /// on `spawn_blocking` (see the type's docs).
    fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: &jsonrpsee::core::params::ArrayParams,
    ) -> Result<T> {
        let mut wait = RETRY_FIRST_WAIT;
        let mut attempt = 1;
        let started = std::time::Instant::now();
        loop {
            match self
                .runtime
                .block_on(self.client.request(method, params.clone()))
            {
                Ok(value) => return Ok(value),
                Err(e)
                    if is_transient(&e)
                        && attempt < RETRY_ATTEMPTS
                        && started.elapsed() + wait < RETRY_DEADLINE =>
                {
                    std::thread::sleep(wait);
                    wait = (wait * 2).min(RETRY_MAX_WAIT);
                    attempt += 1;
                }
                Err(e) => return Err(NodeError::Network(format!("{method}: {e}"))),
            }
        }
    }
}

/// Attempts per request before a transport failure is final. The waits
/// between them add up to about 92 s (0.5, 1, 2, 4, 8, 16, then 20 s each).
const RETRY_ATTEMPTS: u32 = 10;
/// However many attempts remain, a request is given up after this long, so a
/// peer that answers each request slowly cannot hold one for ten timeouts.
const RETRY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(180);
const RETRY_FIRST_WAIT: std::time::Duration = std::time::Duration::from_millis(500);
const RETRY_MAX_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// A failure worth retrying: the request never got an answer (refused,
/// rate-limited, timed out, connection lost), as opposed to an answer. A
/// client that needs a restart is not transient: the same client can never
/// succeed again.
fn is_transient(e: &jsonrpsee::core::ClientError) -> bool {
    use jsonrpsee::core::ClientError;
    matches!(e, ClientError::Transport(_) | ClientError::RequestTimeout)
}

fn unhex(text: &str) -> Result<Vec<u8>> {
    hex::decode(text).map_err(|e| NodeError::Decode(format!("bad hex from peer: {e}")))
}

impl crate::consensus::bft::catchup::CheckpointSource for RpcBootstrapSource {
    fn checkpoint(&self) -> Result<Option<crate::consensus::bft::attest::Checkpoint>> {
        let info: Option<crate::rpc::types::CheckpointInfo> =
            self.call("get_checkpoint", &rpc_params![])?;
        info.map(|i| i.checkpoint()).transpose()
    }

    fn checkpoint_of(
        &self,
        epoch: u64,
    ) -> Result<Option<crate::consensus::bft::attest::Checkpoint>> {
        let info: Option<crate::rpc::types::CheckpointInfo> =
            self.call("get_checkpoint", &rpc_params![epoch])?;
        info.map(|i| i.checkpoint()).transpose()
    }

    fn block(&self, height: u64) -> Result<Block> {
        BootstrapSource::block(self, height)
    }
}

impl BootstrapSource for RpcBootstrapSource {
    fn tip_height(&self) -> Result<u64> {
        self.call("get_tip_height", &rpc_params![])
    }

    fn headers(&self, from: u64, to: u64) -> Result<Vec<BlockHeader>> {
        let headers: Vec<String> = self.call("get_headers", &rpc_params![from, to])?;
        headers
            .iter()
            .map(|text| BlockHeader::from_bytes(&unhex(text)?))
            .collect()
    }

    fn snapshot_manifest(&self) -> Result<SnapshotManifest> {
        let text: String = self.call("get_snapshot_manifest", &rpc_params![])?;
        let manifest = SnapshotManifest::decode(&unhex(&text)?)?;
        *self
            .snapshot_height
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(manifest.height);
        Ok(manifest)
    }

    fn snapshot_chunk(&self, index: usize) -> Result<Vec<u8>> {
        let height = self
            .snapshot_height
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ok_or_else(|| NodeError::Network("chunk asked for before the manifest".into()))?;
        let text: String = self.call("get_snapshot_chunk", &rpc_params![height, index])?;
        unhex(&text)
    }

    fn block(&self, height: u64) -> Result<Block> {
        let info: BlockInfo = self.call("get_block_by_height", &rpc_params![height])?;
        Block::from_bytes(&unhex(&info.raw)?)
    }
}

#[cfg(test)]
mod tests {
    use super::is_transient;
    use jsonrpsee::core::ClientError;
    use jsonrpsee::types::ErrorObjectOwned;

    #[test]
    fn a_refused_or_timed_out_request_is_retried_and_an_answer_is_not() {
        // HTTP 429 from a rate-limited gateway surfaces as a transport error.
        let rate_limited = ClientError::Transport("Request rejected `429`".into());
        assert!(is_transient(&rate_limited));
        assert!(is_transient(&ClientError::RequestTimeout));
        // The peer answered "no": asking again gets the same answer.
        let answered = ClientError::Call(ErrorObjectOwned::owned(
            -32601,
            "no such method",
            None::<()>,
        ));
        assert!(!is_transient(&answered));
        let dead = ClientError::RestartNeeded(std::sync::Arc::new(ClientError::RequestTimeout));
        assert!(!is_transient(&dead), "a dead client is not retried");
    }
}
