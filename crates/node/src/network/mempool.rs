//! Thread-safe transaction pool.
//!
//! ## Locking
//!
//! The map is a [`std::sync::RwLock`], not a `tokio::sync::RwLock`. Every
//! critical section here is a pure in-memory map operation with no `.await`
//! inside it, so the async-aware lock would buy nothing and cost more. The
//! `RocksDB` reads that back validation happen *before* the write lock is taken —
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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_pool(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<TxHash, Transaction>> {
        self.transactions
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The height admission judges against: the block after the stored tip.
    ///
    /// # Errors
    ///
    /// Propagates every read or decode failure.
    fn admission_height(&self) -> Result<u64> {
        // `chain_meta` is asked first because it is the one read that says
        // "no chain yet" as a value (`None`) rather than as an error.
        // `tip_height` reports a missing chain and a failed RocksDB read as the
        // same `NodeError::Storage`, so matching on its error would turn a
        // degraded store into "judge at genesis" — the wrong schedule, the day
        // one has deprecations in it.
        //
        // A store no chain has been opened on judges at genesis: a node still
        // gossips before it has caught up, and nothing it admits moves value
        // until `stage_transaction` re-checks at the block's real height.
        if self.state.chain_meta()?.is_none() {
            return Ok(0);
        }
        Ok(self.state.tip_height()?.saturating_add(1))
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
        // The state checks run *before* the signature, and that is a reversal
        // of what this function used to do, for a cost reason (ADR-013):
        //
        // A v5/v6 hybrid signature costs ~0.18 ms, so verifying first was the
        // cheap way to keep forgeries away from state. A v8 multisig frame can
        // carry sixteen SLH-DSA-SHAKE-256f approvals at ~7.5 ms each, and a
        // forged one is refused only after all of that work. Two RocksDB point
        // reads are a rounding error beside it. So an account that holds
        // nothing and has never sent — which costs an attacker nothing to name
        // — is refused before a single signature is checked, and so is a stale
        // nonce or an unaffordable spend. The sender is derived from the keys
        // (or, for v8, the policy) the frame names, so reading it first trusts
        // nothing: a forged frame reads the account its keys hash to.
        //
        // What this gives up: a forged frame from an empty account now fails as
        // `EmptySender` rather than as a bad signature, so peer scoring sees a
        // refusal, not a forgery. The executor's order is unchanged —
        // `stage_transaction` still verifies before it reads a balance — so
        // nothing about block validity moves.
        // The base fee against committed state (ADR-029): an underpaying
        // transaction could never be built into a block, so pooling and
        // relaying it only costs every peer. The block's own check still runs
        // against the base fee at the height it lands.
        if let Some(fees) = self.state.committed_fees()? {
            let required = fees.required(tx.to_bytes().len());
            let offered: u128 = tx
                .outputs
                .iter()
                .filter(|o| o.recipient == crate::state::fees::FEE_COLLECTOR)
                .map(|o| u128::from(o.amount))
                .sum();
            if offered < required {
                return Err(NodeError::FeeTooLow { required, offered });
            }
        }

        let sender_address = tx.sender();
        let sender = self.state.get_account(&sender_address)?;
        if sender.balance == 0 && sender.nonce == 0 {
            return Err(NodeError::EmptySender {
                address: hex::encode(sender_address),
            });
        }

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

        // Then the signature, with the same rule block execution applies, at
        // the first height this transaction could execute at, under the policy
        // governance has chosen:
        //
        // - Suite-tagged (v7) and multisig (v8) frames are judged here at all.
        //   `verify` has no height, so it refuses them, and a mempool using it
        //   would drop every one before the executor ever saw it.
        // - The height is the tip's successor, the earliest block this
        //   transaction can be included in. Admission is advisory:
        //   `stage_transaction` re-checks at the block's real height.
        // - The policy comes from the governance table, which is what makes
        //   `ParameterKey::DefaultSignatureSuite` a parameter the node reads
        //   rather than a number in a struct. It chooses what wallets should
        //   sign with; it does not decide what verifies. Refusing to relay a
        //   suite that blocks still accept would strand valid transactions
        //   without changing any block's validity, so the admissibility test
        //   here is the one consensus applies.
        let height = self.admission_height()?;
        let policy = crate::crypto::suites::policy(&self.state.parameters()?)?;
        self.state.verify_cached(tx, height, &policy)?;

        // Evidence is stateless to check, so garbage never reaches a block
        // template — the executor checks it again regardless.
        if let crate::core::TxKind::AttestAttack(attestation) = &tx.kind {
            crate::state::threat_exec::verify_evidence(attestation, height)?;
        }
        crate::state::iot_exec::admit(&self.state, &sender_address, &tx.kind)?;

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
