//! Background block indexer.
//!
//! ## Pulling, not listening
//!
//! The node's JSON-RPC has no subscription mechanism, so the indexer asks for
//! the next height until the node says it does not exist yet, then waits. The
//! cost is latency bounded by the poll interval; the benefit is that it works
//! against the node exactly as it is, with no new RPC surface in
//! consensus-adjacent code.
//!
//! ## Reorgs
//!
//! Before extending, the indexer re-checks the block it already has at the tip.
//! If the node now reports a different id at that height, the chain reorganised
//! and the indexed branch is stale. The indexer walks back to the fork point and
//! discards everything above it.
//!
//! Without that check the explorer would keep serving an abandoned branch
//! indefinitely, and would never notice: heights would keep advancing normally.

use std::sync::Arc;
use std::time::Duration;

use custom_l1_node::rpc::BlockInfo;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use jsonrpsee::rpc_params;
use tokio::sync::broadcast;

use crate::error::{ExplorerError, Result};
use crate::model::{IndexedBlock, IndexedTx};
use crate::store::BlockStore;

/// How long to wait when the node has no further blocks.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How far back a reorg may be traced before giving up.
///
/// A fork deeper than this is not a reorg the explorer should silently paper
/// over — it means the indexer is pointed at a different chain, and reindexing
/// from scratch is the honest response.
pub const MAX_REORG_DEPTH: i64 = 128;

/// Capacity of the live-update broadcast channel.
///
/// A slow WebSocket client falls behind and its receiver reports `Lagged`
/// rather than stalling the indexer. Indexing must never block on a browser.
pub const EVENT_CHANNEL_CAPACITY: usize = 256;

/// A live indexing event.
#[derive(Clone, Debug, PartialEq)]
pub enum IndexEvent {
    /// A block was indexed.
    Block(Box<IndexedBlock>),
    /// A transaction was indexed.
    Transaction(Box<IndexedTx>),
    /// Blocks at and above this height were discarded after a reorg.
    Rollback {
        /// Lowest height discarded.
        from_height: i64,
        /// How many blocks were removed.
        removed: u64,
    },
}

/// Reads blocks from a node.
///
/// A trait so the indexer can be driven by a stub in tests without a live node
/// — the ingestion logic is what these tests are about, not HTTP.
#[async_trait::async_trait]
pub trait BlockSource: Send + Sync {
    /// Fetches a block by height, or `None` if the chain has not reached it.
    async fn block_at(&self, height: i64) -> Result<Option<BlockInfo>>;
}

/// A [`BlockSource`] backed by the node's JSON-RPC endpoint.
pub struct RpcSource {
    client: HttpClient,
    url: String,
}

impl RpcSource {
    /// Connects to a node.
    ///
    /// # Errors
    ///
    /// Returns [`ExplorerError::NodeRpc`] if the URL is malformed.
    pub fn connect(url: &str) -> Result<Self> {
        let client = HttpClientBuilder::default()
            .build(url)
            .map_err(|e| ExplorerError::NodeRpc(format!("building client for {url}: {e}")))?;
        Ok(Self {
            client,
            url: url.to_string(),
        })
    }

    /// The endpoint being indexed.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

#[async_trait::async_trait]
impl BlockSource for RpcSource {
    async fn block_at(&self, height: i64) -> Result<Option<BlockInfo>> {
        let response: core::result::Result<BlockInfo, _> = self
            .client
            .request("get_block_by_height", rpc_params![height as u64])
            .await;

        match response {
            Ok(block) => Ok(Some(block)),
            // The node returns an error for a height it has not reached. That
            // is the normal steady state, not a fault, so it maps to `None`
            // rather than propagating and stopping the indexer.
            Err(_) => Ok(None),
        }
    }
}

/// The indexing loop.
pub struct Indexer<S: BlockStore + ?Sized> {
    source: Arc<dyn BlockSource>,
    store: Arc<S>,
    events: broadcast::Sender<IndexEvent>,
    poll_interval: Duration,
}

impl<S: BlockStore + ?Sized> Indexer<S> {
    /// Builds an indexer.
    #[must_use]
    pub fn new(source: Arc<dyn BlockSource>, store: Arc<S>) -> Self {
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            source,
            store,
            events,
            poll_interval: POLL_INTERVAL,
        }
    }

    /// Overrides the poll interval. Tests use a short one.
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Subscribes to live indexing events.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<IndexEvent> {
        self.events.subscribe()
    }

    /// A sender for the event channel, for wiring into an HTTP layer.
    #[must_use]
    pub fn events(&self) -> broadcast::Sender<IndexEvent> {
        self.events.clone()
    }

    /// Indexes one block, publishing it to subscribers.
    async fn index(&self, block: &BlockInfo) -> Result<IndexedBlock> {
        let indexed = IndexedBlock::from_rpc(block);
        let transactions = IndexedTx::from_block(block);

        self.store.put_block(&indexed, &transactions).await?;

        // Send failures mean nobody is subscribed, which is not an error.
        let _ = self
            .events
            .send(IndexEvent::Block(Box::new(indexed.clone())));
        for transaction in transactions {
            let _ = self
                .events
                .send(IndexEvent::Transaction(Box::new(transaction)));
        }

        Ok(indexed)
    }

    /// Detects and unwinds a reorg at the indexed tip.
    ///
    /// Returns the height indexing should resume from.
    async fn reconcile(&self, tip: i64) -> Result<i64> {
        let mut height = tip;
        let floor = (tip - MAX_REORG_DEPTH).max(0);

        while height >= floor {
            let Some(indexed) = self.store.block_by_height(height).await? else {
                // Nothing indexed here; nothing to reconcile.
                return Ok(height);
            };
            let Some(current) = self.source.block_at(height).await? else {
                // The node no longer has this height at all — it reorged to a
                // shorter chain. Discard from here up.
                let removed = self.store.rollback_from(height).await?;
                let _ = self.events.send(IndexEvent::Rollback {
                    from_height: height,
                    removed,
                });
                height -= 1;
                continue;
            };

            if current.header.id == indexed.id {
                // Fork point found: this block still matches, so everything
                // below it is sound and indexing resumes above.
                return Ok(height + 1);
            }

            height -= 1;
        }

        // The divergence is deeper than a reorg should ever go. Discarding
        // silently would hide a misconfiguration, so unwind the whole window
        // and let the operator see the rollback in the event stream.
        let removed = self.store.rollback_from(floor).await?;
        let _ = self.events.send(IndexEvent::Rollback {
            from_height: floor,
            removed,
        });
        Ok(floor)
    }

    /// Advances the index as far as the node allows, then returns.
    ///
    /// Returns how many blocks were indexed. Separate from [`Indexer::run`] so
    /// tests can drive the loop deterministically instead of racing a timer.
    ///
    /// # Errors
    ///
    /// Propagates store and node failures.
    pub async fn sync_once(&self) -> Result<u64> {
        let mut next = match self.store.latest_height().await? {
            // Re-check the tip before extending: if the node now reports a
            // different block there, the branch we indexed is gone.
            Some(tip) => self.reconcile(tip).await?,
            None => 0,
        };

        let mut indexed = 0u64;
        while let Some(block) = self.source.block_at(next).await? {
            self.index(&block).await?;
            indexed += 1;
            next += 1;
        }

        Ok(indexed)
    }

    /// Runs until cancelled, indexing as blocks appear.
    pub async fn run(self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        loop {
            match self.sync_once().await {
                Ok(0) => {}
                Ok(count) => {
                    if let Ok(Some(tip)) = self.store.latest_height().await {
                        println!("indexed {count} block(s), tip now {tip}");
                    }
                }
                // A transient node or database failure must not kill the
                // indexer; it retries on the next tick.
                Err(error) => eprintln!("indexer: {error}"),
            }

            tokio::select! {
                () = tokio::time::sleep(self.poll_interval) => {}
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
    }
}
