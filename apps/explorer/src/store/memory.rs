//! In-memory store.
//!
//! Backs the tests, and doubles as a runnable explorer with no database — which
//! is what makes the ingestion loop, deduplication, and reorg handling testable
//! at all on a machine without PostgreSQL.

use std::collections::BTreeMap;
use std::sync::RwLock;

use async_trait::async_trait;

use crate::error::Result;
use crate::model::{IndexedBlock, IndexedTx};
use crate::store::BlockStore;

/// Indexed data held in memory, ordered by height.
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: RwLock<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    /// Blocks by height. `BTreeMap` so "latest N" is a reverse range scan
    /// rather than a sort, and iteration order is deterministic.
    blocks: BTreeMap<i64, IndexedBlock>,
    /// Transactions by height, in inclusion order.
    transactions: BTreeMap<i64, Vec<IndexedTx>>,
}

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A poisoned lock means a previous holder panicked. The data is plain
    /// maps with no cross-entry invariant a partial write could break, so
    /// recovering beats taking the explorer down.
    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[async_trait]
impl BlockStore for MemoryStore {
    async fn put_block(&self, block: &IndexedBlock, transactions: &[IndexedTx]) -> Result<()> {
        let mut inner = self.write();
        // Insert replaces, so re-indexing a height is idempotent rather than
        // duplicating it.
        inner.blocks.insert(block.height, block.clone());
        inner
            .transactions
            .insert(block.height, transactions.to_vec());
        Ok(())
    }

    async fn latest_height(&self) -> Result<Option<i64>> {
        Ok(self.read().blocks.keys().next_back().copied())
    }

    async fn latest_blocks(&self, limit: i64) -> Result<Vec<IndexedBlock>> {
        let limit = limit.max(0) as usize;
        Ok(self
            .read()
            .blocks
            .values()
            .rev()
            .take(limit)
            .cloned()
            .collect())
    }

    async fn block_by_height(&self, height: i64) -> Result<Option<IndexedBlock>> {
        Ok(self.read().blocks.get(&height).cloned())
    }

    async fn block_by_id(&self, id: &str) -> Result<Option<IndexedBlock>> {
        Ok(self
            .read()
            .blocks
            .values()
            .find(|block| block.id == id)
            .cloned())
    }

    async fn transaction(&self, txid: &str) -> Result<Option<IndexedTx>> {
        Ok(self
            .read()
            .transactions
            .values()
            .flatten()
            .find(|tx| tx.txid == txid)
            .cloned())
    }

    async fn transactions_in_block(&self, height: i64) -> Result<Vec<IndexedTx>> {
        Ok(self
            .read()
            .transactions
            .get(&height)
            .cloned()
            .unwrap_or_default())
    }

    async fn transactions_by_sender(&self, sender: &str, limit: i64) -> Result<Vec<IndexedTx>> {
        let limit = limit.max(0) as usize;
        Ok(self
            .read()
            .transactions
            .values()
            .rev()
            .flatten()
            .filter(|tx| tx.sender == sender)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn block_count(&self) -> Result<i64> {
        Ok(self.read().blocks.len() as i64)
    }

    async fn transaction_count(&self) -> Result<i64> {
        Ok(self
            .read()
            .transactions
            .values()
            .map(Vec::len)
            .sum::<usize>() as i64)
    }

    async fn rollback_from(&self, height: i64) -> Result<u64> {
        let mut inner = self.write();
        let doomed: Vec<i64> = inner
            .blocks
            .range(height..)
            .map(|(height, _)| *height)
            .collect();

        for height in &doomed {
            inner.blocks.remove(height);
            inner.transactions.remove(height);
        }

        Ok(doomed.len() as u64)
    }
}
