//! In-memory ledger, for tests.
//!
//! Proves the logic — window walks, maturity, reversal, batch lifecycle — and
//! proves nothing about durability. Anything that matters about surviving a
//! crash is only demonstrated against [`super::rocks::RocksLedger`].
//!
//! One `Mutex` around one struct, deliberately. Finer-grained locking would buy
//! throughput this type does not need and would let a test observe a state no
//! ordering of the real store can produce.

use std::collections::BTreeMap;
use std::sync::Mutex;

use custom_l1_node::consensus::U256;
use custom_l1_node::state::Address;

use crate::error::{PoolError, Result};
use crate::model::{PayoutBatch, PayoutEntry, PayoutState, ShareRecord};

use super::{BlockState, FoundBlock, MinerBalance, ShareLedger, WindowSlice, accumulate};

/// Everything the ledger holds.
#[derive(Debug, Default)]
struct Inner {
    /// Shares by sequence.
    shares: BTreeMap<u64, ShareRecord>,
    /// Next sequence to issue.
    next_sequence: u64,
    /// Found blocks by id.
    blocks: BTreeMap<String, FoundBlock>,
    /// Credits owed per block, so a reversal knows exactly what to take back.
    ///
    /// Recomputing them from the window at reversal time would be a second
    /// computation over a ledger that has moved on, and it would not agree.
    block_credits: BTreeMap<String, Vec<PayoutEntry>>,
    /// Balances by miner.
    balances: BTreeMap<Address, MinerBalance>,
    /// Payout batches by id.
    batches: BTreeMap<u64, PayoutBatch>,
    /// Next batch id.
    next_batch: u64,
}

/// A ledger that lives in memory.
#[derive(Debug, Default)]
pub struct MemoryLedger {
    /// See [`Inner`].
    inner: Mutex<Inner>,
}

impl MemoryLedger {
    /// An empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Locks the inner state, converting a poisoned lock into a ledger error.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Inner>> {
        self.inner
            .lock()
            .map_err(|_| PoolError::Ledger("in-memory ledger lock was poisoned".to_string()))
    }
}

impl ShareLedger for MemoryLedger {
    fn append_share(
        &self,
        miner: &Address,
        worker: &str,
        weight: u64,
        at_millis: u64,
    ) -> Result<u64> {
        let mut inner = self.lock()?;
        let sequence = inner.next_sequence;
        inner.next_sequence += 1;

        inner.shares.insert(
            sequence,
            ShareRecord {
                sequence,
                miner: *miner,
                worker: worker.to_string(),
                weight,
                accepted_at_millis: at_millis,
            },
        );

        Ok(sequence)
    }

    fn window(&self, from_sequence: u64, target: U256) -> Result<WindowSlice> {
        let inner = self.lock()?;
        let records = inner
            .shares
            .range(..=from_sequence)
            .rev()
            .map(|(_, record)| record.clone());
        Ok(accumulate(records, target))
    }

    fn tip_sequence(&self) -> Result<Option<u64>> {
        let inner = self.lock()?;
        Ok(inner.shares.keys().next_back().copied())
    }

    fn record_block(&self, block: &FoundBlock, credits: &[PayoutEntry]) -> Result<()> {
        let mut inner = self.lock()?;
        if inner.blocks.contains_key(&block.id) {
            // Idempotent. A resubmitted block is one the pool already credited.
            return Ok(());
        }

        for entry in credits {
            let balance = inner.balances.entry(entry.miner).or_default();
            balance.immature = balance.immature.saturating_add(entry.amount);
        }

        inner
            .block_credits
            .insert(block.id.clone(), credits.to_vec());
        inner.blocks.insert(block.id.clone(), block.clone());
        Ok(())
    }

    fn block(&self, id: &str) -> Result<Option<FoundBlock>> {
        Ok(self.lock()?.blocks.get(id).cloned())
    }

    fn immature_blocks(&self) -> Result<Vec<FoundBlock>> {
        let inner = self.lock()?;
        let mut blocks: Vec<_> = inner
            .blocks
            .values()
            .filter(|block| block.state == BlockState::Immature)
            .cloned()
            .collect();
        blocks.sort_by_key(|block| block.height);
        Ok(blocks)
    }

    fn recent_blocks(&self, limit: usize) -> Result<Vec<FoundBlock>> {
        let inner = self.lock()?;
        let mut blocks: Vec<_> = inner.blocks.values().cloned().collect();
        blocks.sort_by_key(|block| std::cmp::Reverse(block.height));
        blocks.truncate(limit);
        Ok(blocks)
    }

    fn mature_block(&self, id: &str) -> Result<()> {
        let mut inner = self.lock()?;
        match inner.blocks.get(id).map(|block| block.state) {
            Some(BlockState::Immature) => {}
            // Already mature, already orphaned, or unknown: nothing to do. This
            // is the idempotency the confirmation watcher depends on.
            _ => return Ok(()),
        }

        let credits = inner.block_credits.get(id).cloned().unwrap_or_default();
        for entry in &credits {
            let balance = inner.balances.entry(entry.miner).or_default();
            balance.immature = balance.immature.saturating_sub(entry.amount);
            balance.unpaid = balance.unpaid.saturating_add(entry.amount);
        }

        if let Some(block) = inner.blocks.get_mut(id) {
            block.state = BlockState::Mature;
        }
        Ok(())
    }

    fn orphan_block(&self, id: &str) -> Result<()> {
        let mut inner = self.lock()?;
        match inner.blocks.get(id).map(|block| block.state) {
            Some(BlockState::Immature) => {}
            _ => return Ok(()),
        }

        let credits = inner.block_credits.get(id).cloned().unwrap_or_default();
        for entry in &credits {
            let balance = inner.balances.entry(entry.miner).or_default();
            balance.immature = balance.immature.saturating_sub(entry.amount);
        }

        if let Some(block) = inner.blocks.get_mut(id) {
            block.state = BlockState::Orphaned;
        }
        Ok(())
    }

    fn balance(&self, miner: &Address) -> Result<MinerBalance> {
        Ok(self
            .lock()?
            .balances
            .get(miner)
            .cloned()
            .unwrap_or_default())
    }

    fn balances(&self) -> Result<Vec<(Address, MinerBalance)>> {
        let inner = self.lock()?;
        Ok(inner
            .balances
            .iter()
            .filter(|(_, balance)| balance.unpaid > 0 || balance.immature > 0 || balance.paid > 0)
            .map(|(address, balance)| (*address, balance.clone()))
            .collect())
    }

    fn next_batch_id(&self) -> Result<u64> {
        let mut inner = self.lock()?;
        let id = inner.next_batch;
        inner.next_batch += 1;
        Ok(id)
    }

    fn create_batch(&self, batch: &PayoutBatch) -> Result<()> {
        let mut inner = self.lock()?;
        if inner.batches.contains_key(&batch.id) {
            return Ok(());
        }

        for entry in &batch.entries {
            let balance = inner.balances.entry(entry.miner).or_default();
            balance.unpaid = balance.unpaid.saturating_sub(entry.amount);
            balance.paid = balance.paid.saturating_add(entry.amount);
        }
        inner.batches.insert(batch.id, batch.clone());
        Ok(())
    }

    fn reverse_batch(&self, batch: &PayoutBatch) -> Result<()> {
        let mut inner = self.lock()?;
        if let Some(stored) = inner.batches.get(&batch.id)
            && matches!(
                stored.state,
                PayoutState::Confirmed | PayoutState::Failed | PayoutState::Orphaned
            )
        {
            // Already terminal. A reorg seen twice must not credit twice.
            return Ok(());
        }

        for entry in &batch.entries {
            let balance = inner.balances.entry(entry.miner).or_default();
            balance.paid = balance.paid.saturating_sub(entry.amount);
            balance.unpaid = balance.unpaid.saturating_add(entry.amount);
        }
        inner.batches.insert(batch.id, batch.clone());
        Ok(())
    }

    fn put_batch(&self, batch: &PayoutBatch) -> Result<()> {
        self.lock()?.batches.insert(batch.id, batch.clone());
        Ok(())
    }

    fn batch(&self, id: u64) -> Result<Option<PayoutBatch>> {
        Ok(self.lock()?.batches.get(&id).cloned())
    }

    fn open_batches(&self) -> Result<Vec<PayoutBatch>> {
        let inner = self.lock()?;
        Ok(inner
            .batches
            .values()
            .filter(|batch| !matches!(batch.state, PayoutState::Confirmed | PayoutState::Failed))
            .cloned()
            .collect())
    }

    fn recent_batches(&self, limit: usize) -> Result<Vec<PayoutBatch>> {
        let inner = self.lock()?;
        Ok(inner.batches.values().rev().take(limit).cloned().collect())
    }

    fn prune_shares_below(&self, sequence: u64) -> Result<u64> {
        let mut inner = self.lock()?;
        let doomed: Vec<u64> = inner
            .shares
            .range(..sequence)
            .map(|(key, _)| *key)
            .collect();
        let count = doomed.len() as u64;
        for key in doomed {
            inner.shares.remove(&key);
        }
        Ok(count)
    }
}
