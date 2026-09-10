//! The share ledger, behind a trait.
//!
//! Two implementations: [`rocks::RocksLedger`] for deployment and
//! [`memory::MemoryLedger`] for tests. The trait exists so the PPLNS window
//! walk, the maturity rules, and the reorg reversal path can be tested without
//! a database — the same reasoning `explorer/src/store/mod.rs` gives for its
//! own split, and the same honest limit applies: `MemoryLedger` proves the
//! logic and cannot prove the durability.
//!
//! ## Why RocksDB and not the explorer's PostgreSQL
//!
//! The two stores hold different kinds of data. Everything the explorer indexes
//! can be rebuilt by re-reading the chain; a share credit exists nowhere but
//! here, and a lost one is a miner who worked for nothing. That argues for the
//! storage engine the operator already runs and already knows how to back up,
//! which on this project is RocksDB.
//!
//! The cost is stated rather than hidden: one node, no replication, and a
//! restore is a file restore.
//!
//! ## The trait is synchronous
//!
//! RocksDB is a blocking API, and wrapping it in `async_trait` would produce
//! futures that block their executor thread anyway — the appearance of async
//! without the property. `src/rpc/server.rs` already calls `StateDB` straight
//! from request handlers for the same reason. Every read a request handler
//! makes here is a point lookup or a short bounded scan; the one unbounded walk
//! ([`ShareLedger::window`]) runs on the payout task, never on a request.
//!
//! ## Append-only, and what that buys
//!
//! Nothing rewrites a [`ShareRecord`]. Credits are derived from the records by
//! reading a window backwards from the tip, and a record that could be edited
//! is a credit that could be moved after the fact. Records are eventually
//! *pruned* — dropped wholesale from the old end — which is a different
//! operation from editing and cannot change what a retained window says.

pub mod memory;
pub mod rocks;

use custom_l1_node::consensus::U256;
use custom_l1_node::state::Address;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::model::{PayoutBatch, PayoutEntry, ShareRecord};

/// Hard ceiling on shares examined by one window walk.
///
/// The PPLNS window is defined by *weight*, not by a share count, and a
/// misconfigured pool — a huge `pplns_factor`, or share targets that collapsed
/// to the floor — could otherwise define a window that walks the entire ledger
/// on every found block. The cap turns that misconfiguration into a short
/// window rather than a stalled payout task.
pub const MAX_WINDOW_SHARES: usize = 5_000_000;

/// Where a found block is in its journey to being payable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockState {
    /// Submitted and accepted, not yet buried deep enough to pay on.
    Immature,
    /// Buried to the configured depth. Its credits are payable.
    Mature,
    /// Lost to a reorg. Its credits were reversed.
    Orphaned,
}

/// A block the pool found.
///
/// Keyed by block id rather than height, and that is the whole point: a reorg
/// replaces the block *at* a height, so a height-keyed record would silently
/// come to describe a different block than the one whose shares were credited.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoundBlock {
    /// Block id, hex.
    pub id: String,
    /// Height it was submitted at.
    pub height: u64,
    /// Ledger sequence of the share that solved it.
    ///
    /// The PPLNS window is measured backwards from here, not from the ledger
    /// tip: shares submitted after the block was found belong to the next
    /// block's window, and paying them from this one would pay twice.
    pub sequence: u64,
    /// Value distributed to miners, after the operator's fee.
    pub reward: u64,
    /// Miner credited with finding it.
    pub finder: Address,
    /// When the pool accepted it, in milliseconds since the epoch.
    pub found_at_millis: u64,
    /// Current state.
    pub state: BlockState,
}

/// One miner's standing with the pool.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinerBalance {
    /// Credits from confirmed blocks, awaiting a payout batch.
    pub unpaid: u64,
    /// Credits from blocks that could still be orphaned.
    pub immature: u64,
    /// Total ever sent on chain.
    pub paid: u64,
}

/// A slice of the ledger, newest first.
#[derive(Clone, Debug, Default)]
pub struct WindowSlice {
    /// The shares in the window, newest first.
    pub shares: Vec<ShareRecord>,
    /// Their total weight.
    ///
    /// `U256` rather than `u64`: a full window is millions of shares, and at
    /// the top of the vardiff band each is worth up to `2^63`. The individual
    /// weights fit a `u64` by construction; their sum does not.
    pub total: U256,
    /// Whether the walk stopped because the window was full rather than because
    /// the ledger ran out.
    ///
    /// A window that is not full means the pool has not yet accumulated `N`
    /// worth of shares, and the split denominator is the accumulated weight
    /// instead. Paying a partial window against the full `N` would pay out less
    /// than the reward and quietly strand the difference.
    pub full: bool,
}

/// Read and write access to share credits and payouts.
pub trait ShareLedger: Send + Sync {
    /// Appends a credited share and returns its sequence number.
    fn append_share(
        &self,
        miner: &Address,
        worker: &str,
        weight: u64,
        at_millis: u64,
    ) -> Result<u64>;

    /// Walks backwards from `from_sequence`, accumulating up to `target` weight.
    ///
    /// Stops at [`MAX_WINDOW_SHARES`] regardless.
    fn window(&self, from_sequence: u64, target: U256) -> Result<WindowSlice>;

    /// Highest sequence number issued, or `None` for an empty ledger.
    fn tip_sequence(&self) -> Result<Option<u64>>;

    /// Records a found block and its credits, atomically.
    ///
    /// Both or neither: a block written without its credits pays nobody, and
    /// credits written without their block can never be matured or reversed.
    fn record_block(&self, block: &FoundBlock, credits: &[PayoutEntry]) -> Result<()>;

    /// A found block by id.
    fn block(&self, id: &str) -> Result<Option<FoundBlock>>;

    /// Blocks still in [`BlockState::Immature`], oldest first.
    fn immature_blocks(&self) -> Result<Vec<FoundBlock>>;

    /// Recent blocks, newest first.
    fn recent_blocks(&self, limit: usize) -> Result<Vec<FoundBlock>>;

    /// Moves a block's credits from immature to unpaid.
    ///
    /// Idempotent: a block already mature is a no-op. The confirmation watcher
    /// can legitimately see the same block cross the threshold twice — after a
    /// restart, or when two polls overlap — and crediting twice would pay
    /// twice.
    fn mature_block(&self, id: &str) -> Result<()>;

    /// Reverses a block's credits and marks it orphaned.
    ///
    /// Idempotent for the same reason as [`ShareLedger::mature_block`], and
    /// consequential in the other direction: a reversal applied twice would
    /// take credits a miner earned on some other block.
    fn orphan_block(&self, id: &str) -> Result<()>;

    /// One miner's balance.
    fn balance(&self, miner: &Address) -> Result<MinerBalance>;

    /// Every miner with a non-zero balance.
    fn balances(&self) -> Result<Vec<(Address, MinerBalance)>>;

    /// Reserves the next batch id.
    fn next_batch_id(&self) -> Result<u64>;

    /// Creates a batch and debits its recipients, atomically.
    ///
    /// The two halves cannot be separate calls. A crash between them would
    /// either leave a batch about to pay balances it never took — paying twice
    /// once the next batch selects the same unpaid credits — or take balances
    /// for a batch that no longer exists. There is no ordering of two writes
    /// that is safe, so this is one write.
    ///
    /// Balances move to `paid` here rather than at confirmation. Optimistic on
    /// purpose: the alternative leaves credits selectable by a second batch
    /// while the first is in flight. [`ShareLedger::reverse_batch`] is what
    /// makes it honest.
    ///
    /// Idempotent — a batch id that already exists is a no-op.
    fn create_batch(&self, batch: &PayoutBatch) -> Result<()>;

    /// Moves a batch to a terminal failure state and returns its balances,
    /// atomically.
    ///
    /// The inverse of [`ShareLedger::create_batch`], and one write for the same
    /// reason. Idempotent: a batch already in a terminal state is a no-op, so a
    /// reorg seen twice does not credit twice.
    fn reverse_batch(&self, batch: &PayoutBatch) -> Result<()>;

    /// Writes a batch record, without touching balances.
    ///
    /// For the state moves that are only about the batch itself —
    /// `Signed → Submitted → Confirmed`. Anything that changes what a miner is
    /// owed goes through [`ShareLedger::create_batch`] or
    /// [`ShareLedger::reverse_batch`].
    fn put_batch(&self, batch: &PayoutBatch) -> Result<()>;

    /// A batch by id.
    fn batch(&self, id: u64) -> Result<Option<PayoutBatch>>;

    /// Batches that have not reached a terminal state, oldest first.
    ///
    /// This is the crash-recovery list. Everything in it either needs
    /// broadcasting or needs watching, and a daemon that skipped it would leave
    /// a signed transaction on disk and a miner unpaid.
    fn open_batches(&self) -> Result<Vec<PayoutBatch>>;

    /// Recent batches, newest first.
    fn recent_batches(&self, limit: usize) -> Result<Vec<PayoutBatch>>;

    /// Drops share records below `sequence`.
    ///
    /// Pruning is wholesale removal from the old end, never an edit: a retained
    /// window says the same thing before and after.
    fn prune_shares_below(&self, sequence: u64) -> Result<u64>;
}

/// Accumulates a window from an iterator of records, newest first.
///
/// Shared by both implementations so the stopping rule — which decides what a
/// miner is paid — exists once rather than twice.
pub(crate) fn accumulate<I>(records: I, target: U256) -> WindowSlice
where
    I: IntoIterator<Item = ShareRecord>,
{
    let mut slice = WindowSlice::default();

    for record in records {
        if slice.shares.len() >= MAX_WINDOW_SHARES {
            slice.full = true;
            break;
        }

        slice.total = slice.total.saturating_add(U256::from_u64(record.weight));
        slice.shares.push(record);

        // The boundary share is included whole rather than pro-rated. Pro-rating
        // it is defensible and is not free: it makes a miner's credit depend on
        // where the window edge happens to fall inside their share, which is
        // one more thing to explain and to get wrong. Including it whole moves
        // at most one share's weight, and the window is millions.
        if slice.total >= target {
            slice.full = true;
            break;
        }
    }

    slice
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sequence: u64, weight: u64) -> ShareRecord {
        ShareRecord {
            sequence,
            miner: [1u8; 32],
            worker: "rig".to_string(),
            weight,
            accepted_at_millis: sequence,
        }
    }

    #[test]
    fn a_window_stops_once_the_target_weight_is_reached() {
        let records: Vec<_> = (0..100).rev().map(|seq| record(seq, 10)).collect();
        let slice = accumulate(records, U256::from_u64(55));

        assert_eq!(slice.shares.len(), 6, "six shares of ten reach fifty-five");
        assert_eq!(slice.total, U256::from_u64(60));
        assert!(slice.full);
    }

    #[test]
    fn a_short_ledger_yields_a_window_that_is_not_full() {
        // The distinction decides the split denominator: a partial window is
        // divided by what accumulated, not by the target, or the reward would
        // be under-distributed.
        let records: Vec<_> = (0..3).rev().map(|seq| record(seq, 10)).collect();
        let slice = accumulate(records, U256::from_u64(1_000));

        assert_eq!(slice.shares.len(), 3);
        assert!(!slice.full);
    }

    #[test]
    fn a_window_total_survives_weights_that_overflow_a_u64() {
        // Two shares at the top of the vardiff band already exceed `u64::MAX`
        // in sum. This is why the total is a `U256`.
        let records = vec![record(1, u64::MAX), record(0, u64::MAX)];
        let slice = accumulate(records, U256::MAX);

        let expected = U256::from_u64(u64::MAX).saturating_add(U256::from_u64(u64::MAX));
        assert_eq!(slice.total, expected);
        assert!(slice.total > U256::from_u64(u64::MAX));
    }

    #[test]
    fn an_empty_ledger_yields_an_empty_window() {
        let slice = accumulate(Vec::new(), U256::from_u64(100));
        assert!(slice.shares.is_empty());
        assert_eq!(slice.total, U256::ZERO);
        assert!(!slice.full);
    }
}
