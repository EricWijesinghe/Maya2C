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
//!
//! ## One sender's share (pre-audit S-013)
//!
//! A nonce may run at most [`MAX_NONCE_AHEAD`] past the account's, one sender
//! holds at most [`MAX_PER_SENDER`] entries, and everything a sender has
//! pooled must be affordable together, not one transaction at a time. Without
//! these, one account holding a single coin filled the pool with
//! future-nonce spends of that coin, none of which ever paid a fee.
//!
//! ## One sponsor's share (ADR-135 P-4)
//!
//! A v13 frame's validity depends on a third party's balance, so a sponsor
//! holds at most [`MAX_SPONSORED_PENDING_PER_SPONSOR`] pooled frames, their
//! `max_fee`s together within its balance, one frame per inner txid, and a
//! sponsored frame is evicted before an unsponsored one at an equal rate.
//! All of it is admission policy: none of it decides whether a block is valid.

use primitive_types::U256;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

use crate::core::sponsor_tx::MAX_SPONSORED_PENDING_PER_SPONSOR;
use crate::core::{MAX_TX_BYTES, Transaction};
use crate::error::{NodeError, Result};
use crate::state::{Address, Module, StateDB};

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
    /// Encoded bytes of everything pooled. A count cap alone lets 4,096
    /// transactions of `MAX_TX_BYTES` each pin about 16 GiB.
    pooled_bytes: Arc<AtomicUsize>,
    max_bytes: usize,
    /// Entries and summed outputs pooled per sender, changed only under the
    /// pool's write lock (always taken first).
    senders: Arc<std::sync::Mutex<HashMap<Address, Pending>>>,
    /// Fee, encoded size and whether it is sponsored, for every pooled entry,
    /// so a full pool can evict its cheapest per byte without re-encoding
    /// everything it holds. Changed only under the pool's write lock.
    fees: Arc<std::sync::Mutex<HashMap<TxHash, Ranked>>>,
    /// Entries and summed `max_fee` pooled per sponsor (ADR-135 P-4), changed
    /// only under the pool's write lock.
    sponsors: Arc<std::sync::Mutex<HashMap<Address, Pending>>>,
    /// The pooled sponsored frame for each inner txid: one per inner, so two
    /// sponsors of one transaction never both wait here (threat model P-12).
    inners: Arc<std::sync::Mutex<HashMap<TxHash, TxHash>>>,
    /// Whether this node admits and relays v13 sponsored frames at all: the
    /// node-local kill switch ADR-135 names, never a validity rule.
    accept_sponsored: bool,
}

/// What eviction ranks a pooled entry by.
#[derive(Clone, Copy, Debug)]
struct Ranked {
    fee: U256,
    size: usize,
    sponsored: bool,
}

/// What one sender has pooled.
#[derive(Clone, Copy, Debug, Default)]
struct Pending {
    count: usize,
    outputs: U256,
}

/// How far past the account's nonce a pooled transaction may run.
pub const MAX_NONCE_AHEAD: u64 = 64;

/// Most pooled transactions one sender may hold.
pub const MAX_PER_SENDER: usize = 64;

/// Default maximum number of pooled transactions.
pub const DEFAULT_CAPACITY: usize = 4_096;

/// Default ceiling on the encoded bytes held in the pool: sixteen of the
/// largest legal transactions, or a few thousand ordinary ones.
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

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
            pooled_bytes: Arc::new(AtomicUsize::new(0)),
            max_bytes: DEFAULT_MAX_BYTES,
            senders: Arc::default(),
            fees: Arc::default(),
            sponsors: Arc::default(),
            inners: Arc::default(),
            accept_sponsored: true,
        }
    }

    /// The same pool with a byte budget of `max_bytes` instead of the default.
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// The same pool admitting v13 sponsored frames or not (`accept_sponsored`,
    /// ADR-135): how an operator stops relaying them without a fork.
    #[must_use]
    pub fn with_accept_sponsored(mut self, accept: bool) -> Self {
        self.accept_sponsored = accept;
        self
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
    fn lock_senders(&self) -> std::sync::MutexGuard<'_, HashMap<Address, Pending>> {
        self.senders
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Refuses `tx` when one more entry from its sender would pass
    /// [`MAX_PER_SENDER`], or its outputs plus the sender's pooled ones would
    /// pass `balance`. Mining shares are exempt: each costs real work.
    fn check_sender_room(&self, tx: &Transaction, balance: U256) -> Result<()> {
        if is_share(tx) {
            return Ok(());
        }
        let sender = tx.sender();
        let pending = self
            .lock_senders()
            .get(&sender)
            .copied()
            .unwrap_or_default();
        if pending.count >= MAX_PER_SENDER {
            return Err(NodeError::MempoolRejected(format!(
                "sender already holds {MAX_PER_SENDER} pooled transactions"
            )));
        }
        let total = pending.outputs.saturating_add(total_out(tx)?);
        if total > balance {
            return Err(NodeError::InsufficientBalance {
                address: hex::encode(sender),
                required: total,
                available: balance,
            });
        }
        Ok(())
    }

    fn lock_fees(&self) -> std::sync::MutexGuard<'_, HashMap<TxHash, Ranked>> {
        self.fees
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_sponsors(&self) -> std::sync::MutexGuard<'_, HashMap<Address, Pending>> {
        self.sponsors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_inners(&self) -> std::sync::MutexGuard<'_, HashMap<TxHash, TxHash>> {
        self.inners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Refuses a sponsored `tx` when its sponsor already has
    /// [`MAX_SPONSORED_PENDING_PER_SPONSOR`] frames pooled, or when their
    /// `max_fee`s plus this one's would pass `sponsor_balance` (ADR-135 P-4,
    /// threat model P-6). Admission policy only: the newest is the one
    /// dropped, and a block holding more is still valid if every frame applies.
    fn check_sponsor_room(&self, tx: &Transaction, sponsor_balance: U256) -> Result<()> {
        let Some(sponsorship) = &tx.sponsor else {
            return Ok(());
        };
        let payer = sponsorship.address();
        let pending = self
            .lock_sponsors()
            .get(&payer)
            .copied()
            .unwrap_or_default();
        if pending.count >= MAX_SPONSORED_PENDING_PER_SPONSOR {
            return Err(NodeError::MempoolRejected(format!(
                "sponsor already holds {MAX_SPONSORED_PENDING_PER_SPONSOR} pooled sponsored frames"
            )));
        }
        // Checked, not saturating: a sum past 2^256 is "cannot cover".
        match pending.outputs.checked_add(sponsorship.max_fee) {
            Some(total) if total <= sponsor_balance => Ok(()),
            total => Err(NodeError::InsufficientBalance {
                address: hex::encode(payer),
                required: total.unwrap_or(U256::MAX),
                available: sponsor_balance,
            }),
        }
    }

    /// The sponsor's committed balance, or zero for an unsponsored frame.
    fn sponsor_balance(&self, tx: &Transaction) -> Result<U256> {
        match &tx.sponsor {
            Some(sponsorship) => Ok(self.state.get_account(&sponsorship.address())?.balance),
            None => Ok(U256::zero()),
        }
    }

    /// What eviction ranks `tx` by. A sponsored frame pays exactly the base
    /// fee on its bytes and no tip, so it ranks at that and, at an equal
    /// rate, is evicted before an unsponsored one (ADR-135 P-4).
    fn ranking_of(&self, tx: &Transaction, size: usize) -> Result<Ranked> {
        let fee = match &tx.sponsor {
            Some(_) => match self.state.committed_fees()? {
                Some(fees) => maya_stateless_core::amount::to_u256(fees.required(size)),
                None => U256::zero(),
            },
            None => fee_of(tx),
        };
        Ok(Ranked {
            fee,
            size,
            sponsored: tx.sponsor.is_some(),
        })
    }

    /// Whether a sponsored frame for the same inner as `tx`, no larger than
    /// `size` bytes, is already pooled (threat model P-12).
    fn holds_smaller_sponsorship(&self, tx: &Transaction, size: usize) -> bool {
        if tx.sponsor.is_none() {
            return false;
        }
        let Some(pooled) = self.lock_inners().get(&tx.inner_txid()).copied() else {
            return false;
        };
        self.lock_fees()
            .get(&pooled)
            .is_some_and(|ranked| ranked.size <= size)
    }

    /// What must leave a pool holding `len` entries and `bytes` for one more
    /// of `size` bytes paying `fee` to fit: its cheapest entries per byte,
    /// each strictly cheaper than the newcomer. Refused when those are not
    /// enough, so a full pool is never displaced by something paying less
    /// (security review of pre-audit S-013: without eviction, 64 senders of
    /// 64 entries each held a full pool against every better-paying sender).
    fn victims(&self, len: usize, bytes: usize, fee: U256, size: usize) -> Result<Vec<TxHash>> {
        let mut count = (len + 1).saturating_sub(self.capacity);
        let mut over = bytes.saturating_add(size).saturating_sub(self.max_bytes);
        if count == 0 && over == 0 {
            return Ok(Vec::new());
        }
        let per_byte = |f: U256, s: usize| f.saturating_mul(U256::from(s));
        let mut ranked: Vec<(TxHash, Ranked)> =
            self.lock_fees().iter().map(|(h, r)| (*h, *r)).collect();
        // Ascending fee per byte (a/b < c/d as a·d < c·b), then sponsored
        // before unsponsored (ADR-135 P-4), then by hash.
        ranked.sort_by(|(ha, a), (hb, b)| {
            per_byte(a.fee, b.size)
                .cmp(&per_byte(b.fee, a.size))
                .then(b.sponsored.cmp(&a.sponsored))
                .then(ha.cmp(hb))
        });
        let mut out = Vec::new();
        for (
            hash,
            Ranked {
                fee: f, size: s, ..
            },
        ) in ranked
        {
            if count == 0 && over == 0 {
                break;
            }
            if per_byte(f, size) >= per_byte(fee, s) {
                break;
            }
            out.push(hash);
            count = count.saturating_sub(1);
            over = over.saturating_sub(s);
        }
        if count > 0 || over > 0 {
            return Err(NodeError::MempoolRejected(format!(
                "pool is full ({} transactions, {} bytes) and nothing pooled pays less per byte",
                self.capacity, self.max_bytes
            )));
        }
        Ok(out)
    }

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
        // A mining share that moves nothing is fee-free and may come from an
        // account that has never held a coin (ADR-044), so neither the fee nor
        // the empty-sender rule below applies. It is admitted only if it would
        // pay right now: right epoch, not yet spent, work meeting the target.
        // Those run first because they are cheaper than the signature.
        // Sponsored first: a sponsored share is paid for, not fee-free.
        if tx.sponsor.is_some() {
            return self.validate_sponsored(tx);
        }
        if let crate::core::TxKind::MiningShare(share) = &tx.kind
            && tx.outputs.is_empty()
        {
            return self.validate_share(tx, share);
        }
        self.validate_unsponsored(tx)
    }

    /// Admission for a v13 sponsored frame (ADR-135 P-4): the node's switch,
    /// the activation height, the consensus fee rules and the admission
    /// window against committed state, the sponsor able to pay this frame,
    /// then the inner frame's own checks with the empty-sender rule lifted —
    /// a sponsored user may hold nothing, which is what sponsorship is for.
    fn validate_sponsored(&self, tx: &Transaction) -> Result<()> {
        let Some(sponsorship) = &tx.sponsor else {
            return self.validate_unsponsored(tx);
        };
        if !self.accept_sponsored {
            return Err(NodeError::MempoolRejected(
                "this node does not relay sponsored (v13) frames".into(),
            ));
        }
        let height = self.admission_height()?;
        crate::state::sponsor::require_active(
            tx,
            crate::state::BlockContext::at_height(height)
                .with_features(self.state.effective_schedule()?.features()),
        )?;
        let fees = self.state.committed_fees()?;
        let required =
            crate::state::sponsor::sponsored_fee(tx, sponsorship, fees.as_ref(), height)?;
        crate::state::sponsor::check_window(sponsorship, height)?;
        let payer = sponsorship.address();
        let available = self.state.get_account(&payer)?.balance;
        if available < required || available.is_zero() {
            return Err(NodeError::InsufficientBalance {
                address: hex::encode(payer),
                required,
                available,
            });
        }
        // A sponsored share is paid for, but still only worth relaying if it
        // would pay its miner: the share rules apply would refuse it on.
        if let crate::core::TxKind::MiningShare(share) = &tx.kind
            && tx.outputs.is_empty()
        {
            self.state.check_mining_share(&tx.sender(), share)?;
        }
        self.validate_inner(tx, false)
    }

    /// Admission for every frame that is neither a fee-free share nor
    /// sponsored: the base fee, then the inner checks.
    fn validate_unsponsored(&self, tx: &Transaction) -> Result<()> {
        if let Some(fees) = self.state.committed_fees()? {
            let required = maya_stateless_core::amount::to_u256(fees.required(tx.to_bytes().len()));
            let offered = tx
                .outputs
                .iter()
                .filter(|o| o.recipient == crate::state::fees::FEE_COLLECTOR)
                .fold(U256::zero(), |sum, o| sum.saturating_add(o.amount));
            if offered < required {
                return Err(NodeError::FeeTooLow { required, offered });
            }
        }
        self.validate_inner(tx, true)
    }

    /// The checks every non-share frame shares: nonce, balance, vault, the
    /// signature(s), evidence, activation and the breaker. `empty_sender`
    /// refuses an account that has never held or sent anything, before its
    /// signature costs anything; a sponsored frame lifts it, because its
    /// sponsor was checked instead.
    fn validate_inner(&self, tx: &Transaction, empty_sender: bool) -> Result<()> {
        let sender_address = tx.sender();
        let sender = self.state.get_account(&sender_address)?;
        if empty_sender && sender.balance.is_zero() && sender.nonce == 0 {
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
        if tx.nonce > sender.nonce.saturating_add(MAX_NONCE_AHEAD) {
            return Err(NodeError::MempoolRejected(format!(
                "nonce {} is more than {MAX_NONCE_AHEAD} past the account's {}",
                tx.nonce, sender.nonce
            )));
        }

        let total_out = total_out(tx)?;
        if sender.balance < total_out {
            return Err(NodeError::InsufficientBalance {
                address: hex::encode(sender_address),
                required: total_out,
                available: sender.balance,
            });
        }

        // A vault account's out-of-policy transaction (ADR-030) would only be
        // dropped at block building; refuse it here, before the signature,
        // with the reason, against committed state like the checks above.
        self.state.vault_check(
            &crate::state::db::Overlay::default(),
            tx,
            &sender_address,
            self.admission_height()?,
        )?;

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
        let sunsets = self.state.effective_schedule()?.sunsets;
        let policy = crate::crypto::suites::policy(&self.state.parameters()?, &sunsets)?;
        self.state.verify_cached(tx, height, &policy)?;

        // Evidence is stateless to check, so garbage never reaches a block
        // template — the executor checks it again regardless. Chain tag is required
        // for signature verification (ADR-036).
        if let crate::core::TxKind::AttestAttack(attestation) = &tx.kind {
            // Admission only: the executor re-checks under the committed schedule
            // (ADR-082), and a mempool refusal is advisory.
            crate::state::threat_exec::verify_evidence(
                attestation,
                height,
                self.state.get_chain_tag()?,
                &sunsets,
            )?;
        }
        crate::state::iot_exec::admit(&self.state, &sender_address, &tx.kind)?;
        // The same activation check the executor makes (ADR-135), at the
        // first height this transaction could land at.
        let context = crate::state::BlockContext::at_height(height)
            .with_features(self.state.effective_schedule()?.features());
        crate::state::asset_mint::require_active(&tx.kind, context)?;
        crate::state::nft::require_nft_active(&tx.kind, context)?;
        crate::state::names::require_names_active(&tx.kind, context)?;

        self.check_breaker(&tx.kind)
    }

    /// Admission for a fee-free mining share (ADR-044). Cheapest first: a
    /// stale nonce, then the share rules against committed state (epoch, spent
    /// set, the 2 ms proof of work), and only then the signature. A flood of
    /// shares that do not pay is refused before any of it reaches a block
    /// template, and getting one admitted costs real work.
    fn validate_share(
        &self,
        tx: &Transaction,
        share: &crate::core::mining_payload::MiningShare,
    ) -> Result<()> {
        let miner = tx.sender();
        let account = self.state.get_account(&miner)?;
        if tx.nonce < account.nonce {
            return Err(NodeError::InvalidNonce {
                address: hex::encode(miner),
                expected: account.nonce,
                actual: tx.nonce,
            });
        }
        self.state.check_mining_share(&miner, share)?;
        let height = self.admission_height()?;
        let sunsets = self.state.effective_schedule()?.sunsets;
        let policy = crate::crypto::suites::policy(&self.state.parameters()?, &sunsets)?;
        self.state.verify_cached(tx, height, &policy)?;
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

        // Room first (pre-audit S-016): refusing for a full pool or a sender
        // at its share must not cost a signature check. Re-checked under the
        // write lock below, since another insert may land in between.
        let balance = self.state.get_account(&tx.sender())?.balance;
        let sponsor_balance = self.sponsor_balance(&tx)?;
        let size = tx.to_bytes().len();
        // The decoder refuses a frame this large, but a local submission never
        // is decoded. A transaction over the maximum could never fit a block.
        if size > MAX_TX_BYTES {
            return Err(NodeError::MempoolRejected(format!(
                "transaction of {size} bytes exceeds the maximum {MAX_TX_BYTES}"
            )));
        }
        let ranked = self.ranking_of(&tx, size)?;
        let fee = ranked.fee;
        if self.holds_smaller_sponsorship(&tx, size) {
            return Ok(false);
        }
        self.victims(
            self.len(),
            self.pooled_bytes.load(Ordering::Relaxed),
            fee,
            size,
        )?;
        self.check_sender_room(&tx, balance)?;
        self.check_sponsor_room(&tx, sponsor_balance)?;

        // Validation performs RocksDB reads; deliberately outside any lock.
        self.validate(&tx)?;

        let mut pool = self.write_pool();
        if pool.contains_key(&hash) || self.holds_smaller_sponsorship(&tx, size) {
            return Ok(false);
        }
        self.check_sender_room(&tx, balance)?;
        self.check_sponsor_room(&tx, sponsor_balance)?;
        // One sponsored frame per inner (P-12): a smaller outer frame for the
        // same inner replaces the pooled one, so the sponsor that asks less
        // of the block is the one kept. Removed only after every check that
        // can refuse the newcomer, the eviction plan included, so a refused
        // newcomer never costs the pooled frame its place. The plan counts
        // the old frame as still pooled: conservative, never over capacity.
        // Read into a local first: `forget` takes the inners lock itself.
        let replaced = match &tx.sponsor {
            Some(_) => self.lock_inners().get(&tx.inner_txid()).copied(),
            None => None,
        };
        // Every mutation holds the write lock, so this read cannot race an add.
        let bytes = self.pooled_bytes.load(Ordering::Relaxed);
        let victims = self.victims(pool.len(), bytes, fee, size)?;
        if let Some(old_hash) = replaced
            && let Some(old) = pool.remove(&old_hash)
        {
            self.forget(&old_hash, &old);
        }
        for victim in victims {
            if let Some(old) = pool.remove(&victim) {
                self.forget(&victim, &old);
            }
        }

        self.track(&tx, true);
        self.lock_fees().insert(hash, ranked);
        pool.insert(hash, tx);
        self.pooled_bytes.fetch_add(size, Ordering::Relaxed);
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

    /// Pooled transactions and their encoded bytes, read under one lock so the
    /// two agree. Read-only: the sentinel's stranding check uses it.
    #[must_use]
    pub fn usage(&self) -> (usize, usize) {
        let pool = self.read_pool();
        let bytes = pool.values().map(|tx| tx.to_bytes().len()).sum();
        (pool.len(), bytes)
    }

    /// Whether the pool is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read_pool().is_empty()
    }

    /// Applies `pick` to every pooled transaction under one read lock and keeps
    /// what it returns, without cloning the transactions themselves. A pooled
    /// transaction is ~13 KB, so [`Mempool::snapshot`] every few milliseconds on a
    /// busy pool copies tens of megabytes to look at a hash.
    pub fn scan<T>(&self, mut pick: impl FnMut(&TxHash, &Transaction) -> Option<T>) -> Vec<T> {
        self.read_pool()
            .iter()
            .filter_map(|(hash, tx)| pick(hash, tx))
            .collect()
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
        let mut removed = 0;
        for hash in hashes {
            if let Some(tx) = pool.remove(hash) {
                self.forget(hash, &tx);
                removed += 1;
            }
        }
        removed
    }

    /// Takes a removed entry out of the byte count and both tallies. Called
    /// with the pool's write lock held.
    fn forget(&self, hash: &TxHash, tx: &Transaction) {
        self.pooled_bytes
            .fetch_sub(tx.to_bytes().len(), Ordering::Relaxed);
        self.track(tx, false);
        self.lock_fees().remove(hash);
    }

    /// Adds `tx` to its sender's [`Pending`] or takes it away. Called with the
    /// pool's write lock held, so the tally always matches the pool.
    fn track(&self, tx: &Transaction, added: bool) {
        if is_share(tx) {
            return;
        }
        // Admitted transactions passed `total_out`, so this cannot overflow.
        let outputs = total_out(tx).unwrap_or_default();
        tally(&mut self.lock_senders(), tx.sender(), outputs, added);
        if let Some(sponsorship) = &tx.sponsor {
            // The sponsor's tally is in `max_fee`s: what it signed to cover.
            tally(
                &mut self.lock_sponsors(),
                sponsorship.address(),
                sponsorship.max_fee,
                added,
            );
            let mut inners = self.lock_inners();
            if added {
                inners.insert(tx.inner_txid(), tx.txid());
            } else if inners.get(&tx.inner_txid()) == Some(&tx.txid()) {
                inners.remove(&tx.inner_txid());
            }
        }
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

/// What `tx` pays the fee collector.
fn fee_of(tx: &Transaction) -> U256 {
    tx.outputs
        .iter()
        .filter(|o| o.recipient == crate::state::fees::FEE_COLLECTOR)
        .fold(U256::zero(), |sum, o| sum.saturating_add(o.amount))
}

/// A fee-free mining share (ADR-044), which moves nothing. A sponsored one
/// is not fee-free: its sponsor pays, so it takes the sponsored path.
fn is_share(tx: &Transaction) -> bool {
    matches!(tx.kind, crate::core::TxKind::MiningShare(_))
        && tx.outputs.is_empty()
        && tx.sponsor.is_none()
}

/// Adds `amount` to `key`'s [`Pending`] in `map`, or takes it away and
/// drops the entry at zero.
fn tally(map: &mut HashMap<Address, Pending>, key: Address, amount: U256, added: bool) {
    let entry = map.entry(key).or_default();
    if added {
        entry.count += 1;
        entry.outputs = entry.outputs.saturating_add(amount);
    } else {
        entry.count = entry.count.saturating_sub(1);
        entry.outputs = entry.outputs.saturating_sub(amount);
        if entry.count == 0 {
            map.remove(&key);
        }
    }
}

/// The sum of `tx`'s outputs.
///
/// # Errors
///
/// [`NodeError::BalanceOverflow`] when they do not fit in 256 bits.
fn total_out(tx: &Transaction) -> Result<U256> {
    tx.outputs.iter().try_fold(U256::zero(), |sum, output| {
        sum.checked_add(output.amount)
            .ok_or(NodeError::BalanceOverflow)
    })
}
