//! Indexer and store behaviour.
//!
//! Driven by a stub [`BlockSource`] rather than a live node, so ingestion
//! logic — deduplication, reorg detection, rollback — is tested deterministically
//! instead of by racing a real chain. A separate test drives the same code
//! against an actual node process.

use std::sync::Arc;
use std::sync::Mutex;

use custom_l1_node::rpc::{BlockInfo, HeaderInfo};
use maya_explorer::error::Result;
use maya_explorer::indexer::{BlockSource, IndexEvent, Indexer};
use maya_explorer::model::{IndexedBlock, average_block_time, estimate_hashrate};
use maya_explorer::store::BlockStore;
use maya_explorer::store::memory::MemoryStore;

// ---------------------------------------------------------------------------
// stub chain
// ---------------------------------------------------------------------------

/// A chain the test controls block by block.
#[derive(Default)]
struct StubChain {
    blocks: Mutex<Vec<BlockInfo>>,
}

impl StubChain {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Appends a block whose id encodes `(height, variant)`.
    ///
    /// The variant lets a test replace a height with a *different* block, which
    /// is exactly what a reorg does.
    fn push(&self, height: u64, variant: u8, tx_count: usize) {
        let mut blocks = self.blocks.lock().expect("lock");
        let block = BlockInfo {
            height,
            header: HeaderInfo {
                id: format!("{height:016x}{variant:02x}"),
                prev_hash: format!("{:016x}00", height.saturating_sub(1)),
                state_root: "00".repeat(32),
                timestamp: 1_700_000_000 + height * 15,
                nonce: height,
                // 8 leading zero bits: small enough that the derived work is a
                // number a test can reason about.
                difficulty_target: format!("00{}", "ff".repeat(31)),
            },
            transactions: (0..tx_count)
                .map(|index| custom_l1_node::rpc::TransactionInfo {
                    txid: format!("{height:08x}{index:08x}{}", "0".repeat(48)),
                    sender: "aa".repeat(32),
                    lattice_public_key: "cc".repeat(1952),
                    hash_public_key: "dd".repeat(32),
                    nonce: index as u64,
                    outputs: vec![custom_l1_node::rpc::OutputInfo {
                        recipient: "bb".repeat(32),
                        amount: 100,
                    }],
                    signed: true,
                })
                .collect(),
            raw: String::new(),
        };

        if (height as usize) < blocks.len() {
            blocks[height as usize] = block;
        } else {
            blocks.push(block);
        }
    }

    /// Drops every block at or above `height`, simulating a shorter chain.
    fn truncate(&self, height: usize) {
        self.blocks.lock().expect("lock").truncate(height);
    }
}

#[async_trait::async_trait]
impl BlockSource for StubChain {
    async fn block_at(&self, height: i64) -> Result<Option<BlockInfo>> {
        Ok(self
            .blocks
            .lock()
            .expect("lock")
            .get(height as usize)
            .cloned())
    }
}

fn indexer(chain: Arc<StubChain>, store: Arc<MemoryStore>) -> Indexer<MemoryStore> {
    Indexer::new(chain, store)
}

// ---------------------------------------------------------------------------
// store
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_empty_store_has_no_tip() {
    let store = MemoryStore::new();
    assert_eq!(store.latest_height().await.expect("query"), None);
    assert_eq!(store.block_count().await.expect("query"), 0);
    assert!(store.latest_blocks(10).await.expect("query").is_empty());
}

#[tokio::test]
async fn re_indexing_a_height_replaces_rather_than_duplicates() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    chain.push(0, 0, 1);

    let indexer = indexer(Arc::clone(&chain), Arc::clone(&store));
    indexer.sync_once().await.expect("first");

    // A restart re-reads a height it already has. Idempotency is what keeps
    // that from doubling every row.
    let indexer = indexer_from(&chain, &store);
    indexer.sync_once().await.expect("second");

    assert_eq!(store.block_count().await.expect("count"), 1);
    assert_eq!(store.transaction_count().await.expect("count"), 1);
}

fn indexer_from(chain: &Arc<StubChain>, store: &Arc<MemoryStore>) -> Indexer<MemoryStore> {
    Indexer::new(Arc::clone(chain) as Arc<dyn BlockSource>, Arc::clone(store))
}

#[tokio::test]
async fn latest_blocks_returns_newest_first() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    for height in 0..5 {
        chain.push(height, 0, 0);
    }

    indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("sync");

    let latest = store.latest_blocks(3).await.expect("query");
    assert_eq!(latest.len(), 3);
    assert_eq!(latest[0].height, 4);
    assert_eq!(latest[2].height, 2);
}

// ---------------------------------------------------------------------------
// ingestion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_indexer_ingests_every_available_block() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    for height in 0..10 {
        chain.push(height, 0, 2);
    }

    let indexed = indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("sync");

    assert_eq!(indexed, 10);
    assert_eq!(store.latest_height().await.expect("tip"), Some(9));
    assert_eq!(store.transaction_count().await.expect("count"), 20);
}

#[tokio::test]
async fn the_indexer_resumes_from_where_it_stopped() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    for height in 0..3 {
        chain.push(height, 0, 0);
    }
    assert_eq!(
        indexer_from(&chain, &store).sync_once().await.expect("a"),
        3
    );

    // The chain advances; the indexer must pick up only what is new.
    for height in 3..7 {
        chain.push(height, 0, 0);
    }
    assert_eq!(
        indexer_from(&chain, &store).sync_once().await.expect("b"),
        4
    );
    assert_eq!(store.latest_height().await.expect("tip"), Some(6));
}

#[tokio::test]
async fn syncing_an_empty_chain_indexes_nothing() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    assert_eq!(
        indexer_from(&chain, &store)
            .sync_once()
            .await
            .expect("sync"),
        0
    );
    assert_eq!(store.latest_height().await.expect("tip"), None);
}

#[tokio::test]
async fn transactions_are_indexed_against_their_block() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    chain.push(0, 0, 3);
    indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("sync");

    let transactions = store.transactions_in_block(0).await.expect("query");
    assert_eq!(transactions.len(), 3);
    assert!(transactions.iter().all(|tx| tx.height == 0));
    assert!(transactions.iter().all(|tx| tx.signed));

    let found = store
        .transaction(&transactions[1].txid)
        .await
        .expect("query");
    assert_eq!(found.as_ref(), Some(&transactions[1]));
}

#[tokio::test]
async fn transactions_can_be_listed_by_sender() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    for height in 0..3 {
        chain.push(height, 0, 2);
    }
    indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("sync");

    let sender = "aa".repeat(32);
    let sent = store
        .transactions_by_sender(&sender, 10)
        .await
        .expect("query");
    assert_eq!(sent.len(), 6);

    assert!(
        store
            .transactions_by_sender(&"cc".repeat(32), 10)
            .await
            .expect("query")
            .is_empty()
    );
}

// ---------------------------------------------------------------------------
// reorgs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replaced_tip_is_detected_and_reindexed() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    for height in 0..5 {
        chain.push(height, 0, 0);
    }
    indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("sync");

    let original = store
        .block_by_height(4)
        .await
        .expect("query")
        .expect("block");

    // The chain reorgs: height 4 now holds a different block.
    chain.push(4, 1, 0);
    indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("resync");

    let replaced = store
        .block_by_height(4)
        .await
        .expect("query")
        .expect("block");
    assert_ne!(replaced.id, original.id, "the stale block is still indexed");
    assert_eq!(store.latest_height().await.expect("tip"), Some(4));
}

#[tokio::test]
async fn a_deep_reorg_discards_the_whole_abandoned_branch() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    for height in 0..10 {
        chain.push(height, 0, 1);
    }
    indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("sync");
    assert_eq!(store.block_count().await.expect("count"), 10);

    // Blocks 5 upward are replaced by a different branch of the same length.
    for height in 5..10 {
        chain.push(height, 7, 1);
    }

    indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("resync");

    // Everything from the fork point on must now be the new branch.
    for height in 5..10 {
        let block = store
            .block_by_height(height)
            .await
            .expect("query")
            .expect("block");
        assert!(
            block.id.ends_with("07"),
            "height {height} still holds the abandoned block"
        );
    }
    // And the pre-fork history is untouched.
    for height in 0..5 {
        let block = store
            .block_by_height(height)
            .await
            .expect("query")
            .expect("block");
        assert!(block.id.ends_with("00"));
    }
}

#[tokio::test]
async fn a_shorter_chain_rolls_the_index_back() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    for height in 0..8 {
        chain.push(height, 0, 1);
    }
    indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("sync");

    // The node now reports a chain three blocks shorter.
    chain.truncate(5);
    indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("resync");

    assert_eq!(store.latest_height().await.expect("tip"), Some(4));
    assert_eq!(store.block_count().await.expect("count"), 5);
    // Orphaned transactions must go with their blocks.
    assert_eq!(store.transaction_count().await.expect("count"), 5);
}

#[tokio::test]
async fn a_rollback_is_announced_to_subscribers() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    for height in 0..5 {
        chain.push(height, 0, 0);
    }
    indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("sync");

    let indexer = indexer_from(&chain, &store);
    let mut events = indexer.subscribe();

    chain.truncate(3);
    indexer.sync_once().await.expect("resync");

    // Clients showing an abandoned block need to be told, not left with it.
    let mut saw_rollback = false;
    while let Ok(event) = events.try_recv() {
        if matches!(event, IndexEvent::Rollback { .. }) {
            saw_rollback = true;
        }
    }
    assert!(saw_rollback, "a rollback was not announced");
}

#[tokio::test]
async fn indexed_blocks_are_published_to_subscribers() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    let indexer = indexer_from(&chain, &store);
    let mut events = indexer.subscribe();

    chain.push(0, 0, 2);
    indexer.sync_once().await.expect("sync");

    let mut blocks = 0;
    let mut transactions = 0;
    while let Ok(event) = events.try_recv() {
        match event {
            IndexEvent::Block(_) => blocks += 1,
            IndexEvent::Transaction(_) => transactions += 1,
            IndexEvent::Rollback { .. } => {}
        }
    }
    assert_eq!(blocks, 1);
    assert_eq!(transactions, 2);
}

// ---------------------------------------------------------------------------
// derived metrics
// ---------------------------------------------------------------------------

fn sample(height: i64, timestamp: i64, work: &str) -> IndexedBlock {
    IndexedBlock {
        height,
        id: format!("{height:064x}"),
        prev_hash: String::new(),
        state_root: String::new(),
        timestamp,
        nonce: 0,
        difficulty_target: String::new(),
        tx_count: 0,
        work: work.to_string(),
    }
}

#[tokio::test]
async fn hashrate_needs_at_least_two_blocks() {
    // One block spans no interval, so there is no rate to report.
    assert_eq!(estimate_hashrate(&[]), 0.0);
    assert_eq!(estimate_hashrate(&[sample(1, 100, "1000")]), 0.0);
}

#[tokio::test]
async fn hashrate_is_work_over_elapsed_time() {
    let window = vec![
        sample(1, 100, "1000"),
        sample(2, 110, "1000"),
        sample(3, 120, "1000"),
    ];
    // Two blocks found across 20 seconds: the first block's work predates the
    // window and does not count toward the rate.
    assert!((estimate_hashrate(&window) - 100.0).abs() < 1e-9);
}

#[tokio::test]
async fn genesis_is_excluded_from_rate_windows() {
    // Genesis carries an operator-pinned timestamp, not an observation of when
    // a block was found. On a real chain this gap was months, which dragged the
    // measured average block time to ten million seconds.
    let with_genesis = vec![
        sample(0, 0, "1000"),
        sample(1, 1_000_000, "1000"),
        sample(2, 1_000_015, "1000"),
        sample(3, 1_000_030, "1000"),
    ];
    let without_genesis = &with_genesis[1..];

    assert_eq!(
        estimate_hashrate(&with_genesis),
        estimate_hashrate(without_genesis),
        "genesis must not influence the hashrate estimate"
    );
    assert!(
        (average_block_time(&with_genesis) - 15.0).abs() < 1e-9,
        "expected 15s intervals, got {}",
        average_block_time(&with_genesis)
    );
}

#[tokio::test]
async fn a_chain_with_only_genesis_reports_no_rate() {
    // One configured block and nothing mined: there is nothing to measure.
    let window = vec![sample(0, 1_700_000_000, "1000")];
    assert_eq!(estimate_hashrate(&window), 0.0);
    assert_eq!(average_block_time(&window), 0.0);
}

#[tokio::test]
async fn a_zero_length_window_reports_no_hashrate() {
    // Timestamps are miner-supplied and only loosely ordered, so a window can
    // legitimately appear instantaneous. That must not become an infinity.
    let window = vec![sample(1, 100, "1000"), sample(2, 100, "1000")];
    assert_eq!(estimate_hashrate(&window), 0.0);
    assert!(estimate_hashrate(&window).is_finite());
}

#[tokio::test]
async fn average_block_time_divides_span_by_intervals() {
    let window = vec![
        sample(1, 0, "1"),
        sample(2, 15, "1"),
        sample(3, 30, "1"),
        sample(4, 45, "1"),
    ];
    // Three intervals across 45 seconds.
    assert!((average_block_time(&window) - 15.0).abs() < 1e-9);
    assert_eq!(average_block_time(&window[..1]), 0.0);
}

#[tokio::test]
async fn work_is_derived_from_the_difficulty_target() {
    let chain = StubChain::new();
    let store = Arc::new(MemoryStore::new());
    chain.push(0, 0, 0);
    indexer_from(&chain, &store)
        .sync_once()
        .await
        .expect("sync");

    let block = store
        .block_by_height(0)
        .await
        .expect("query")
        .expect("block");
    // 8 leading zero bits means roughly 2^8 expected hashes.
    let work: f64 = block.work.parse().expect("numeric work");
    assert!(
        (250.0..270.0).contains(&work),
        "work {work} is not near 2^8 for an 8-bit target"
    );
}
