//! The durable share ledger.
//!
//! ## Key layout
//!
//! One column family, prefixed keys, matching `src/state/db.rs` rather than
//! inventing a second convention in the same repository:
//!
//! ```text
//! s<sequence be64>      ShareRecord          the append-only share log
//! b<block id>           FoundBlock           blocks the pool found
//! c<block id>           Vec<PayoutEntry>     what that block credited, verbatim
//! a<address>            MinerBalance         unpaid / immature / paid
//! p<batch id be64>      PayoutBatch          payout batches
//! m:sequence            u64                  next share sequence
//! m:batch               u64                  next batch id
//! ```
//!
//! Sequences and batch ids are stored **big-endian** so `RocksDB`'s byte order is
//! numeric order. With little-endian keys, share 256 would sort before share 2
//! and every window walk would read the wrong shares — silently, and in a way
//! that only shows up as miners being paid the wrong amounts.
//!
//! ## What is atomic, and why those things
//!
//! Three operations write more than one key and each uses a single
//! [`WriteBatch`]:
//!
//! - **recording a block** writes the block, its credit list, and every affected
//!   balance. A block stored without its credits pays nobody; credits stored
//!   without their block can never be matured or reversed.
//! - **maturing** and **orphaning** move balances and flip the block's state
//!   together, so a crash cannot leave a block marked mature whose credits are
//!   still immature — or worse, the reverse.
//! - **debiting a batch** moves every recipient at once.
//!
//! The credit list is stored rather than recomputed. Recomputing it at maturity
//! would re-walk a ledger that has moved on, and the second answer would not
//! match the first.

use std::path::Path;

use custom_l1_node::consensus::U256;
use custom_l1_node::state::Address;
use rocksdb::{DB, IteratorMode, Options, WriteBatch};

use crate::error::{PoolError, Result};
use crate::model::{PayoutBatch, PayoutEntry, PayoutState, ShareRecord};

use super::{BlockState, FoundBlock, MinerBalance, ShareLedger, WindowSlice, accumulate};

/// Prefix for share records.
const SHARE: u8 = b's';
/// Prefix for found blocks.
const BLOCK: u8 = b'b';
/// Prefix for a block's credit list.
const CREDITS: u8 = b'c';
/// Prefix for miner balances.
const BALANCE: u8 = b'a';
/// Prefix for payout batches.
const BATCH: u8 = b'p';

/// Counter holding the next share sequence.
const NEXT_SEQUENCE: &[u8] = b"m:sequence";
/// Counter holding the next batch id.
const NEXT_BATCH: &[u8] = b"m:batch";

/// A ledger backed by `RocksDB`.
#[derive(Debug)]
pub struct RocksLedger {
    /// The open database.
    db: DB,
    /// Serialises the read-modify-write on the two counters.
    ///
    /// `RocksDB` gives atomic *writes*, not atomic read-modify-write, and
    /// [`RocksLedger::bump`] is both. Without this lock two validator threads
    /// accepting shares at the same moment can read the same sequence, and the
    /// second `put` overwrites the first miner's share record with the second
    /// miner's — a credit silently transferred between accounts. The 50-worker
    /// simulation is what surfaced it; a single-threaded test never could.
    ///
    /// A `Mutex<()>` rather than an atomic counter because the value has to
    /// survive a restart, so the durable store is the source of truth and the
    /// lock exists only to make reading and advancing it one step.
    counters: std::sync::Mutex<()>,
}

/// Builds a prefixed key from a byte slice.
fn key(prefix: u8, rest: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + rest.len());
    out.push(prefix);
    out.extend_from_slice(rest);
    out
}

/// Builds a prefixed key from a big-endian counter.
fn counter_key(prefix: u8, value: u64) -> Vec<u8> {
    key(prefix, &value.to_be_bytes())
}

/// Encodes a value as JSON.
///
/// JSON rather than a hand-rolled binary encoding, and the trade is deliberate:
/// this store is written once per share and read in bulk once per block, so the
/// cost is small, while a bespoke codec here would be a second serialization
/// format in a repository that already maintains one for consensus — and this
/// one would be the untested copy.
fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|e| PoolError::Ledger(format!("encode: {e}")))
}

/// Decodes a value from JSON.
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|e| PoolError::Ledger(format!("decode: {e}")))
}

impl RocksLedger {
    /// Opens (or creates) a ledger at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::Ledger`] if the database cannot be opened.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut options = Options::default();
        options.create_if_missing(true);
        let db = DB::open(&options, path)?;
        Ok(Self {
            db,
            counters: std::sync::Mutex::new(()),
        })
    }

    /// Reads and increments a counter.
    ///
    /// Held under [`RocksLedger::counters`] for the whole read-modify-write.
    fn bump(&self, name: &[u8]) -> Result<u64> {
        let _guard = self
            .counters
            .lock()
            .map_err(|_| PoolError::Ledger("the counter lock is poisoned".to_string()))?;

        let current = match self.db.get(name)? {
            Some(bytes) => {
                let array: [u8; 8] = bytes.as_slice().try_into().map_err(|_| {
                    PoolError::Ledger(format!(
                        "counter {} is {} bytes, expected 8",
                        String::from_utf8_lossy(name),
                        bytes.len()
                    ))
                })?;
                u64::from_be_bytes(array)
            }
            None => 0,
        };

        self.db.put(name, (current + 1).to_be_bytes())?;
        Ok(current)
    }

    /// Reads one miner's balance.
    fn read_balance(&self, miner: &Address) -> Result<MinerBalance> {
        match self.db.get(key(BALANCE, miner))? {
            Some(bytes) => decode(&bytes),
            None => Ok(MinerBalance::default()),
        }
    }

    /// Stages a balance write.
    fn stage_balance(
        &self,
        batch: &mut WriteBatch,
        miner: &Address,
        balance: &MinerBalance,
    ) -> Result<()> {
        batch.put(key(BALANCE, miner), encode(balance)?);
        Ok(())
    }

    /// Reads a block's stored credit list.
    fn read_credits(&self, id: &str) -> Result<Vec<PayoutEntry>> {
        match self.db.get(key(CREDITS, id.as_bytes()))? {
            Some(bytes) => decode(&bytes),
            None => Ok(Vec::new()),
        }
    }

    /// Every value stored under `prefix`, in key order.
    fn scan<T: serde::de::DeserializeOwned>(&self, prefix: u8) -> Result<Vec<T>> {
        let start = [prefix];
        let mode = IteratorMode::From(&start, rocksdb::Direction::Forward);

        let mut out = Vec::new();
        for item in self.db.iterator(mode) {
            let (stored_key, value) = item?;
            if stored_key.first() != Some(&prefix) {
                break;
            }
            out.push(decode(&value)?);
        }
        Ok(out)
    }

    /// Moves a block between states, applying `adjust` to each credited miner.
    ///
    /// Both halves land in one [`WriteBatch`], so a crash cannot leave a block
    /// marked settled whose balances are not.
    fn transition(
        &self,
        id: &str,
        to: BlockState,
        adjust: impl Fn(&mut MinerBalance, u64),
    ) -> Result<()> {
        let Some(mut block) = self.block(id)? else {
            return Ok(());
        };
        if block.state != BlockState::Immature {
            // Already settled one way or the other. The idempotency the
            // confirmation watcher depends on.
            return Ok(());
        }

        let mut write = WriteBatch::default();
        for entry in self.read_credits(id)? {
            let mut balance = self.read_balance(&entry.miner)?;
            adjust(&mut balance, entry.amount);
            self.stage_balance(&mut write, &entry.miner, &balance)?;
        }

        block.state = to;
        write.put(key(BLOCK, id.as_bytes()), encode(&block)?);
        self.db.write(write)?;
        Ok(())
    }
}

impl ShareLedger for RocksLedger {
    fn append_share(
        &self,
        miner: &Address,
        worker: &str,
        weight: u64,
        at_millis: u64,
    ) -> Result<u64> {
        let sequence = self.bump(NEXT_SEQUENCE)?;
        let record = ShareRecord {
            sequence,
            miner: *miner,
            worker: worker.to_string(),
            weight,
            accepted_at_millis: at_millis,
        };

        self.db
            .put(counter_key(SHARE, sequence), encode(&record)?)?;
        Ok(sequence)
    }

    fn window(&self, from_sequence: u64, target: U256) -> Result<WindowSlice> {
        // Reverse iteration from the found share, which is why sequences are
        // big-endian: RocksDB orders bytes, and only a big-endian key makes
        // byte order and numeric order the same thing.
        let start = counter_key(SHARE, from_sequence);
        let mode = IteratorMode::From(&start, rocksdb::Direction::Reverse);

        let mut records = Vec::new();
        for item in self.db.iterator(mode) {
            let (stored_key, value) = item?;
            if stored_key.first() != Some(&SHARE) {
                break;
            }
            records.push(decode::<ShareRecord>(&value)?);
            if records.len() >= super::MAX_WINDOW_SHARES {
                break;
            }
        }

        Ok(accumulate(records, target))
    }

    fn tip_sequence(&self) -> Result<Option<u64>> {
        match self.db.get(NEXT_SEQUENCE)? {
            Some(bytes) => {
                let array: [u8; 8] = bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| PoolError::Ledger("sequence counter is corrupt".to_string()))?;
                Ok(u64::from_be_bytes(array).checked_sub(1))
            }
            None => Ok(None),
        }
    }

    fn record_block(&self, block: &FoundBlock, credits: &[PayoutEntry]) -> Result<()> {
        if self.block(&block.id)?.is_some() {
            // A block the pool already credited. Submitting one twice is
            // ordinary — a retry, a restart — and crediting twice is not.
            return Ok(());
        }

        let mut write = WriteBatch::default();
        for entry in credits {
            let mut balance = self.read_balance(&entry.miner)?;
            balance.immature = balance.immature.saturating_add(entry.amount);
            self.stage_balance(&mut write, &entry.miner, &balance)?;
        }

        write.put(key(CREDITS, block.id.as_bytes()), encode(&credits)?);
        write.put(key(BLOCK, block.id.as_bytes()), encode(block)?);
        self.db.write(write)?;
        Ok(())
    }

    fn block(&self, id: &str) -> Result<Option<FoundBlock>> {
        match self.db.get(key(BLOCK, id.as_bytes()))? {
            Some(bytes) => Ok(Some(decode(&bytes)?)),
            None => Ok(None),
        }
    }

    fn immature_blocks(&self) -> Result<Vec<FoundBlock>> {
        let mut blocks: Vec<FoundBlock> = self.scan(BLOCK)?;
        blocks.retain(|block| block.state == BlockState::Immature);
        blocks.sort_by_key(|block| block.height);
        Ok(blocks)
    }

    fn recent_blocks(&self, limit: usize) -> Result<Vec<FoundBlock>> {
        let mut blocks: Vec<FoundBlock> = self.scan(BLOCK)?;
        blocks.sort_by_key(|block| std::cmp::Reverse(block.height));
        blocks.truncate(limit);
        Ok(blocks)
    }

    fn mature_block(&self, id: &str) -> Result<()> {
        self.transition(id, BlockState::Mature, |balance, amount| {
            balance.immature = balance.immature.saturating_sub(amount);
            balance.unpaid = balance.unpaid.saturating_add(amount);
        })
    }

    fn orphan_block(&self, id: &str) -> Result<()> {
        self.transition(id, BlockState::Orphaned, |balance, amount| {
            balance.immature = balance.immature.saturating_sub(amount);
        })
    }

    fn balance(&self, miner: &Address) -> Result<MinerBalance> {
        self.read_balance(miner)
    }

    fn balances(&self) -> Result<Vec<(Address, MinerBalance)>> {
        let start = [BALANCE];
        let mode = IteratorMode::From(&start, rocksdb::Direction::Forward);

        let mut out = Vec::new();
        for item in self.db.iterator(mode) {
            let (stored_key, value) = item?;
            if stored_key.first() != Some(&BALANCE) {
                break;
            }
            let address: Address = stored_key[1..]
                .try_into()
                .map_err(|_| PoolError::Ledger("balance key is not an address".to_string()))?;
            out.push((address, decode::<MinerBalance>(&value)?));
        }
        Ok(out)
    }

    fn next_batch_id(&self) -> Result<u64> {
        self.bump(NEXT_BATCH)
    }

    fn create_batch(&self, batch: &PayoutBatch) -> Result<()> {
        if self.batch(batch.id)?.is_some() {
            return Ok(());
        }

        let mut write = WriteBatch::default();
        for entry in &batch.entries {
            let mut balance = self.read_balance(&entry.miner)?;
            balance.unpaid = balance.unpaid.saturating_sub(entry.amount);
            balance.paid = balance.paid.saturating_add(entry.amount);
            self.stage_balance(&mut write, &entry.miner, &balance)?;
        }

        write.put(counter_key(BATCH, batch.id), encode(batch)?);
        self.db.write(write)?;
        Ok(())
    }

    fn reverse_batch(&self, batch: &PayoutBatch) -> Result<()> {
        if let Some(stored) = self.batch(batch.id)?
            && matches!(
                stored.state,
                PayoutState::Confirmed | PayoutState::Failed | PayoutState::Orphaned
            )
        {
            return Ok(());
        }

        let mut write = WriteBatch::default();
        for entry in &batch.entries {
            let mut balance = self.read_balance(&entry.miner)?;
            balance.paid = balance.paid.saturating_sub(entry.amount);
            balance.unpaid = balance.unpaid.saturating_add(entry.amount);
            self.stage_balance(&mut write, &entry.miner, &balance)?;
        }

        write.put(counter_key(BATCH, batch.id), encode(batch)?);
        self.db.write(write)?;
        Ok(())
    }

    fn put_batch(&self, batch: &PayoutBatch) -> Result<()> {
        self.db.put(counter_key(BATCH, batch.id), encode(batch)?)?;
        Ok(())
    }

    fn batch(&self, id: u64) -> Result<Option<PayoutBatch>> {
        match self.db.get(counter_key(BATCH, id))? {
            Some(bytes) => Ok(Some(decode(&bytes)?)),
            None => Ok(None),
        }
    }

    fn open_batches(&self) -> Result<Vec<PayoutBatch>> {
        let mut batches: Vec<PayoutBatch> = self.scan(BATCH)?;
        batches
            .retain(|batch| !matches!(batch.state, PayoutState::Confirmed | PayoutState::Failed));
        batches.sort_by_key(|batch| batch.id);
        Ok(batches)
    }

    fn recent_batches(&self, limit: usize) -> Result<Vec<PayoutBatch>> {
        let mut batches: Vec<PayoutBatch> = self.scan(BATCH)?;
        batches.sort_by_key(|batch| std::cmp::Reverse(batch.id));
        batches.truncate(limit);
        Ok(batches)
    }

    fn prune_shares_below(&self, sequence: u64) -> Result<u64> {
        let start = [SHARE];
        let mode = IteratorMode::From(&start, rocksdb::Direction::Forward);

        let mut write = WriteBatch::default();
        let mut removed = 0u64;
        for item in self.db.iterator(mode) {
            let (stored_key, _) = item?;
            if stored_key.first() != Some(&SHARE) {
                break;
            }
            let array: [u8; 8] = stored_key[1..]
                .try_into()
                .map_err(|_| PoolError::Ledger("share key is not a sequence".to_string()))?;
            if u64::from_be_bytes(array) >= sequence {
                break;
            }
            write.delete(stored_key);
            removed += 1;
        }

        self.db.write(write)?;
        Ok(removed)
    }
}
