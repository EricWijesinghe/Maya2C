//! Storage behind a trait.
//!
//! Two implementations: [`postgres::PostgresStore`] for deployment and
//! [`memory::MemoryStore`] for tests. The trait exists so indexer logic can be
//! tested without a database — otherwise the ingestion loop, deduplication, and
//! reorg handling would all be untested assertion.
//!
//! ## Honest scope
//!
//! `MemoryStore` proves the *logic*. It cannot prove the SQL: query syntax,
//! index behaviour, and constraint enforcement are only exercised against a
//! real PostgreSQL server, which `tests/postgres_store_tests.rs` does when
//! `MAYA_EXPLORER_TEST_PG` names one (CI's nextest job does).

pub mod memory;
pub mod postgres;

use async_trait::async_trait;

use crate::error::Result;
use crate::model::{Direction, IndexedBlock, IndexedOutput, IndexedTx};

/// A keyset position: a page continues strictly before this row.
///
/// Rows order by `(height, position)`, `position` being the transaction's
/// index within its block. Block pages use the height alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Seek {
    /// Block height.
    pub height: i64,
    /// Transaction index within the block; zero for block pages.
    pub position: i64,
}

/// Read and write access to indexed chain data.
///
/// `async_trait` rather than native async fns: handlers hold an
/// `Arc<dyn BlockStore>` so the store can be swapped without making every
/// route generic, and native async fns in traits are not yet dyn-compatible.
#[async_trait]
pub trait BlockStore: Send + Sync {
    /// Inserts a block and its transactions.
    ///
    /// Must be idempotent. The indexer can legitimately see the same height
    /// twice — after a restart, or when a poll overlaps a previous one — and
    /// re-indexing must not duplicate rows or fail.
    async fn put_block(&self, block: &IndexedBlock, transactions: &[IndexedTx]) -> Result<()>;

    /// Highest indexed height, or `None` when nothing is indexed.
    async fn latest_height(&self) -> Result<Option<i64>>;

    /// The most recent `limit` blocks, newest first.
    async fn latest_blocks(&self, limit: i64) -> Result<Vec<IndexedBlock>>;

    /// A block by height.
    async fn block_by_height(&self, height: i64) -> Result<Option<IndexedBlock>>;

    /// A block by hex id.
    async fn block_by_id(&self, id: &str) -> Result<Option<IndexedBlock>>;

    /// A transaction by hex id.
    async fn transaction(&self, txid: &str) -> Result<Option<IndexedTx>>;

    /// Transactions in a block, in inclusion order.
    async fn transactions_in_block(&self, height: i64) -> Result<Vec<IndexedTx>>;

    /// Transactions sent by an address, newest first.
    async fn transactions_by_sender(&self, sender: &str, limit: i64) -> Result<Vec<IndexedTx>>;

    /// Blocks strictly below `before` (or from the tip when `None`), newest
    /// first, at most `limit`.
    async fn blocks_page(&self, before: Option<Seek>, limit: i64) -> Result<Vec<IndexedBlock>>;

    /// Transactions strictly before `before` in `(height, position)` order,
    /// newest first, at most `limit`. Each comes with its own position, which
    /// is the cursor for the page after it.
    async fn transactions_page(
        &self,
        before: Option<Seek>,
        limit: i64,
    ) -> Result<Vec<(Seek, IndexedTx)>>;

    /// Transactions an address sent, received or both, newest first, keyset
    /// paged as [`BlockStore::transactions_page`]. A transaction that is both
    /// appears once.
    async fn transactions_by_address(
        &self,
        address: &str,
        direction: Direction,
        before: Option<Seek>,
        limit: i64,
    ) -> Result<Vec<(Seek, IndexedTx)>>;

    /// The outputs of a transaction, in output order.
    async fn outputs_of(&self, txid: &str) -> Result<Vec<IndexedOutput>>;

    /// The lowest indexed height, or `None` when nothing is indexed.
    async fn oldest_height(&self) -> Result<Option<i64>>;

    /// Total indexed blocks.
    async fn block_count(&self) -> Result<i64>;

    /// Total indexed transactions.
    async fn transaction_count(&self) -> Result<i64>;

    /// Discards every block at or above `height`.
    ///
    /// Called when the node reports a different block at a height already
    /// indexed, which means a reorg replaced that history. Without it the
    /// explorer would keep serving a branch the chain has abandoned.
    async fn rollback_from(&self, height: i64) -> Result<u64>;
}
