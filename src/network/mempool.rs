//! Thread-safe transaction pool.
//!
//! ## Locking
//!
//! The map is a [`std::sync::RwLock`], not a `tokio::sync::RwLock`. Every
//! critical section here is a pure in-memory map operation with no `.await`
//! inside it, so the async-aware lock would buy nothing and cost more. The
//! RocksDB reads that back validation happen *before* the write lock is taken —
//! holding a std lock across blocking I/O is exactly the mistake this ordering
//! avoids.
//!
//! ## Validation
//!
//! Inbound gossip is untrusted. A transaction is admitted only if it is
//! well-formed, correctly signed, not already pooled, and consistent with
//! committed state: nonce not stale, balance sufficient.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::core::Transaction;
use crate::error::{NodeError, Result};
use crate::state::{Module, StateDB};

/// Identifier of a pooled transaction.
pub type TxHash = [u8; 32];

/// Shared, validated pool of pending transactions.
///
/// Cloning shares the same underlying pool and state handle.
#[derive(Clone)]
pub struct Mempool {
    transactions: Arc<RwLock<HashMap<TxHash, Transaction>>>,
    state: Arc<StateDB>,
    capacity: usize,
}

/// Default maximum number of pooled transactions.
pub const DEFAULT_CAPACITY: usize = 4_096;

impl Mempool {
    /// Creates a mempool backed by `state`.
    #[must_use]
    pub fn new(state: Arc<StateDB>) -> Self {
        Self::with_capacity(state, DEFAULT_CAPACITY)
    }

    /// Creates a mempool holding at most `capacity` transactions.
    #[must_use]
    pub fn with_capacity(state: Arc<StateDB>, capacity: usize) -> Self {
        Self {
            transactions: Arc::new(RwLock::new(HashMap::new())),
            state,
            capacity,
        }
    }

    /// A poisoned lock means another thread panicked while holding it. Rather
    /// than propagate the panic, recover the guard: the map is a plain
    /// key-value store with no cross-entry invariant that a partial write could
    /// have broken.
    fn read_pool(&self) -> std::sync::RwLockReadGuard<'_, HashMap<TxHash, Transaction>> {
        self.transactions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_pool(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<TxHash, Transaction>> {
        self.transactions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Validates `tx` against committed state without modifying the pool.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::SignatureVerification`],
    /// [`NodeError::HashSignatureVerification`] or
    /// [`NodeError::MissingSignature`] for bad authorization,
    /// [`NodeError::InvalidNonce`] for a stale nonce,
    /// [`NodeError::InsufficientBalance`] when the sender cannot cover the
    /// outputs, or [`NodeError::BalanceOverflow`] on arithmetic overflow.
    pub fn validate(&self, tx: &Transaction) -> Result<()> {
        // Authorization before anything else: never let an unsigned or forged
        // transaction reach the state lookups. Both signature schemes must
        // pass, exactly as at block execution — the mempool applies the same
        // rule earlier, it does not apply a weaker one.
        //
        // Doing it first also matters more than it used to. Verifying a hybrid
        // signature costs ~0.18 ms against ML-DSA's ~0.10 ms, so admission is
        // now the cheapest place on the node to spend that, and the most
        // important place not to spend it twice.
        tx.verify()?;

        // Evidence is stateless to check, so garbage never reaches a block
        // template — the executor checks it again regardless.
        if let crate::core::TxKind::AttestAttack(attestation) = &tx.kind {
            crate::state::threat_exec::verify_evidence(attestation)?;
        }
        crate::state::iot_exec::admit(&self.state, &tx.sender(), &tx.kind)?;

        let sender_address = tx.sender();
        let sender = self.state.get_account(&sender_address)?;

        // A nonce below the account's next expected value is a replay of
        // something already committed. Nonces *above* it are accepted: the
        // gap-filling transaction may still be in flight, and the block
        // executor enforces exact ordering at commit time.
        if tx.nonce < sender.nonce {
            return Err(NodeError::InvalidNonce {
                address: hex::encode(sender_address),
                expected: sender.nonce,
                actual: tx.nonce,
            });
        }

        let mut total_out: u64 = 0;
        for output in &tx.outputs {
            total_out = total_out
                .checked_add(output.amount)
                .ok_or(NodeError::BalanceOverflow)?;
        }

        if sender.balance < total_out {
            return Err(NodeError::InsufficientBalance {
                address: hex::encode(sender_address),
                required: total_out,
                available: sender.balance,
            });
        }

        self.check_breaker(&tx.kind)
    }

    /// Refuses a payload whose module the circuit breaker has halted.
    ///
    /// Admission, not consensus: the block executor applies the same gate, and
    /// this only stops the pool accumulating transactions for a block that
    /// would refuse them. It reads the tip height rather than the height the
    /// transaction will land at, which is the closest thing the pool has and
    /// is off by at most the breaker's last block.
    ///
    /// A transaction whose module is halted is rejected rather than held: the
    /// sender can resubmit after the breaker clears, and a pool that held
    /// everything for a hundred blocks would be a queue an attacker can fill.
    fn check_breaker(&self, kind: &crate::core::TxKind) -> Result<()> {
        let Some(module) = Module::of(kind) else {
            return Ok(());
        };
        let Some(record) = self.state.stored_breaker(module)? else {
            return Ok(());
        };
        if record.holds_at(self.state.tip_height()?) {
            return Err(NodeError::ModuleHalted {
                module: module.label(),
                invariant: record.invariant.label(),
                tripped_at: record.tripped_at,
                until: record.until,
            });
        }
        Ok(())
    }

    /// Validates and inserts a transaction.
    ///
    /// Returns `Ok(true)` when the transaction was newly admitted and
    /// `Ok(false)` when it was already pooled — a duplicate is normal in a
    /// gossip mesh, not an error.
    ///
    /// # Errors
    ///
    /// Propagates validation failures, or returns
    /// [`NodeError::MempoolRejected`] when the pool is full.
    pub fn insert(&self, tx: Transaction) -> Result<bool> {
        let hash = tx.txid();

        // Cheap duplicate check before the expensive signature verification, so
        // repeated gossip of a known transaction costs almost nothing.
        if self.read_pool().contains_key(&hash) {
            return Ok(false);
        }

        // Validation performs RocksDB reads; deliberately outside any lock.
        self.validate(&tx)?;

        let mut pool = self.write_pool();
        if pool.contains_key(&hash) {
            return Ok(false);
        }
        if pool.len() >= self.capacity {
            return Err(NodeError::MempoolRejected(format!(
                "pool is full ({} transactions)",
                self.capacity
            )));
        }

        pool.insert(hash, tx);
        Ok(true)
    }

    /// Decodes, validates, and inserts a transaction received from the wire.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a malformed frame, otherwise the same
    /// errors as [`Mempool::insert`].
    pub fn insert_encoded(&self, bytes: &[u8]) -> Result<bool> {
        let tx = Transaction::from_bytes(bytes)?;
        self.insert(tx)
    }

    /// Returns the pooled transaction with the given hash, if present.
    #[must_use]
    pub fn get(&self, hash: &TxHash) -> Option<Transaction> {
        self.read_pool().get(hash).cloned()
    }

    /// Returns `true` if the pool holds this transaction.
    #[must_use]
    pub fn contains(&self, hash: &TxHash) -> bool {
        self.read_pool().contains_key(hash)
    }

    /// Number of pooled transactions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.read_pool().len()
    }

    /// Whether the pool is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read_pool().is_empty()
    }

    /// Snapshot of all pooled transactions.
    #[must_use]
    pub fn snapshot(&self) -> Vec<Transaction> {
        self.read_pool().values().cloned().collect()
    }

    /// Removes transactions by hash, returning how many were present.
    ///
    /// Called after a block commits to evict transactions it included.
    pub fn remove_all(&self, hashes: &[TxHash]) -> usize {
        let mut pool = self.write_pool();
        hashes
            .iter()
            .filter(|hash| pool.remove(*hash).is_some())
            .count()
    }

    /// Drops pooled transactions that committed state has invalidated.
    ///
    /// Run after a block commits: nonces advance and balances fall, so
    /// previously valid entries can become permanently unusable.
    ///
    /// # Errors
    ///
    /// Propagates read failures from [`StateDB`].
    pub fn revalidate(&self) -> Result<usize> {
        let candidates: Vec<(TxHash, Transaction)> = self
            .read_pool()
            .iter()
            .map(|(hash, tx)| (*hash, tx.clone()))
            .collect();

        // Validate outside the lock, then apply the evictions in one pass.
        let stale: Vec<TxHash> = candidates
            .into_iter()
            .filter(|(_, tx)| self.validate(tx).is_err())
            .map(|(hash, _)| hash)
            .collect();

        Ok(self.remove_all(&stale))
    }
}
