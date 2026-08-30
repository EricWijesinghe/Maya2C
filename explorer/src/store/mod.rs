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
//! real PostgreSQL server.

pub mod memory;
pub mod postgres;

use async_trait::async_trait;

use crate::error::Result;
use crate::model::{IndexedBlock, IndexedTx};

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
