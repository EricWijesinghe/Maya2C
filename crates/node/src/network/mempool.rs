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
//! ## Entries that can never execute (independent audit CT-01, CT-04)
//!
//! A pooled entry the executor drops pays no fee, and the BFT feed proposes
//! whatever is pooled, so an entry that can never execute would be ordered
//! and dropped every round. Two rules keep stale, gapped and unaffordable
//! entries out of the pool:
//!
//! - A nonce is admitted only when every nonce from the account's up to it is
//!   already pooled, here or in the node's other pool ([`Mempool::link_sibling`]),
//!   so no gap can be created; and [`Mempool::prune_invalid`] removes a dropped
//!   entry that committed state has made stale, or whose run lost its head.
//! - A fee-free mining share is pooled once per piece of work: the same work
//!   under another transaction nonce could never be paid twice.
//!
//! Not covered: an entry that passes every state check here but fails on its
//! payload when staged (a cancel of an unknown order, a swap on a missing
//! pool) is still re-proposed each round. Pruning it needs the builder to
//! tell a staging failure from an entry left out for block room; BACKLOG.md.

use primitive_types::U256;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

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
    /// Fee and encoded size of every pooled entry, so a full pool can evict
    /// its cheapest per byte without re-encoding everything it holds. Changed
    /// only under the pool's write lock.
    fees: Arc<std::sync::Mutex<HashMap<TxHash, (U256, usize)>>>,
    /// The other local pool's sender tally, once linked: a node feeds both
    /// its gossip pool and its RPC pool to consensus, so a nonce pooled in
    /// either one is no gap ([`Mempool::link_sibling`]).
    sibling: Arc<std::sync::OnceLock<Senders>>,
}

/// Per-sender tallies of one pool.
type Senders = Arc<std::sync::Mutex<HashMap<Address, Pending>>>;

/// What one sender has pooled.
#[derive(Clone, Debug, Default)]
struct Pending {
    /// Every pooled entry, mining shares included.
    count: usize,
    outputs: U256,
    /// How many pooled entries use each nonce; several may compete for one.
    nonces: BTreeMap<u64, usize>,
    /// `(epoch, work nonce)` of each pooled mining share: one piece of work.
    shares: BTreeSet<(u64, u64)>,
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
            sibling: Arc::default(),
        }
    }

    /// Lets this pool and `other` each count the other's pooled nonces in the
    /// gap rule. A node runs a gossip pool and an RPC pool and proposes from
    /// both, so a wallet that sends nonce `n` to one node and `n + 1` to
    /// another has them in different pools here; unlinked, `n + 1` would be
    /// refused as a gap. Only nonces are shared: the per-sender cap and the
    /// affordability sum stay per pool. Returns `false` when either pool was
    /// already linked, and then that pool keeps its first link.
    #[must_use]
    pub fn link_sibling(&self, other: &Self) -> bool {
        let here = self.sibling.set(Arc::clone(&other.senders)).is_ok();
        let there = other.sibling.set(Arc::clone(&self.senders)).is_ok();
        here && there
    }

    /// The same pool with a byte budget of `max_bytes` instead of the default.
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
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
    /// pass `balance`, or when it is a mining share whose work the sender
    /// already has pooled under another transaction nonce. A share moves
    /// nothing, so it has no balance test.
    fn check_sender_room(&self, tx: &Transaction, balance: U256) -> Result<()> {
        let sender = tx.sender();
        let work = share_work(tx);
        let (count, outputs, work_pooled) =
            self.lock_senders()
                .get(&sender)
                .map_or((0, U256::zero(), false), |p| {
                    let pooled = work
                        .as_ref()
                        .is_some_and(|w| w.iter().any(|piece| p.shares.contains(piece)));
                    (p.count, p.outputs, pooled)
                });
        if count >= MAX_PER_SENDER {
            return Err(NodeError::MempoolRejected(format!(
                "sender already holds {MAX_PER_SENDER} pooled transactions"
            )));
        }
        if work_pooled {
            return Err(NodeError::MempoolRejected(
                "work in this share batch is already pooled under another nonce".into(),
            ));
        }
        if work.is_some() {
            return Ok(());
        }
        let total = outputs.saturating_add(total_out(tx)?);
        if total > balance {
            return Err(NodeError::InsufficientBalance {
                address: hex::encode(sender),
                required: total,
                available: balance,
            });
        }
        Ok(())
    }

    /// Refuses a nonce that would leave a gap: every nonce from the account's
    /// up to `tx_nonce` must already be pooled. The executor runs a sender's
    /// transactions at exact nonces only, so an entry past a gap would be
    /// ordered and dropped, fee-free, every round until the gap filled.
    ///
    /// A nonce pooled in the linked sibling pool counts ([`Mempool::link_sibling`]),
    /// so a sender who used two RPC endpoints is not refused here.
    ///
    /// Cost: a sender whose nonces reach this node out of order over gossip
    /// has the early arrivals refused here, and gossip does not redeliver
    /// them; the node they were sent to, and peers that saw them in order,
    /// still hold them.
    fn check_no_gap(&self, sender: &Address, account_nonce: u64, tx_nonce: u64) -> Result<()> {
        if tx_nonce > account_nonce.saturating_add(MAX_NONCE_AHEAD) {
            return Err(NodeError::MempoolRejected(format!(
                "nonce {tx_nonce} is more than {MAX_NONCE_AHEAD} past the account's {account_nonce}"
            )));
        }
        // At most MAX_NONCE_AHEAD lookups, by the window check above. The two
        // tallies are locked one after the other, never together, so two
        // linked pools checking at once cannot deadlock.
        let absent = |tally: &HashMap<Address, Pending>, nonces: &mut Vec<u64>| {
            let pooled = tally.get(sender).map(|p| &p.nonces);
            nonces.retain(|nonce| pooled.is_none_or(|n| !n.contains_key(nonce)));
        };
        let mut missing: Vec<u64> = (account_nonce..tx_nonce).collect();
        absent(&self.lock_senders(), &mut missing);
        if let Some(sibling) = self.sibling.get()
            && !missing.is_empty()
        {
            absent(
                &sibling
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                &mut missing,
            );
        }
        if let Some(&missing) = missing.first() {
            return Err(NodeError::MempoolRejected(format!(
                "nonce {tx_nonce} leaves a gap: nonce {missing} is not pooled"
            )));
        }
        Ok(())
    }

    fn lock_fees(&self) -> std::sync::MutexGuard<'_, HashMap<TxHash, (U256, usize)>> {
        self.fees
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        let mut ranked: Vec<(TxHash, U256, usize)> = self
            .lock_fees()
            .iter()
            .map(|(h, &(f, s))| (*h, f, s))
            .collect();
        // Ascending fee per byte (a/b < c/d as a·d < c·b), ties by hash.
        ranked.sort_by(|a, b| {
            per_byte(a.1, b.2)
                .cmp(&per_byte(b.1, a.2))
                .then(a.0.cmp(&b.0))
        });
        let mut out = Vec::new();
        for (hash, f, s) in ranked {
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
        if let crate::core::TxKind::MiningShare(share) = &tx.kind
            && is_share(tx)
        {
            return self.validate_share(tx, share);
        }
        // A work claim is never pooled or relayed until an Accepted ADR binds
        // it to a verified share (ADR-044). The executor refuses it as well.
        if matches!(tx.kind, crate::core::TxKind::ClaimWork(_))
            && !crate::upgrade::WORK_CLAIM_ADR_ACCEPTED
        {
            return Err(NodeError::WorkClaimInactive {
                height: self.admission_height()?,
                activation: crate::state::context::WORK_CLAIM_ACTIVATION_HEIGHT,
            });
        }
        let sender_address = self.check_against_state(tx)?;

        // A contract call over the gas cap, or a deployment whose length alone
        // no block could hold, can never be included (ADR-142, GAS-1). Refused
        // before the signature: constant work, far below a signature's.
        self.state.admit_vm_gas(&tx.kind)?;

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
        // A deployment's shape is a scan of the whole module: only for a
        // signed one (PR #549 review).
        self.state.admit_deploy_shape(&tx.kind)?;

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

        self.check_breaker(&tx.kind)
    }

    /// The checks of [`Mempool::validate`] that read committed state, short of
    /// the signature: the base fee, an empty sender, a stale nonce or a gap, an
    /// unaffordable spend, a vault's policy. Returns the sender.
    ///
    /// A signature verified at admission cannot go bad later, so these are
    /// what can turn a pooled entry stale; [`Mempool::prune_invalid`] runs
    /// only these.
    fn check_against_state(&self, tx: &Transaction) -> Result<Address> {
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

        let sender_address = tx.sender();
        let sender = self.state.get_account(&sender_address)?;
        if sender.balance.is_zero() && sender.nonce == 0 {
            return Err(NodeError::EmptySender {
                address: hex::encode(sender_address),
            });
        }

        // A nonce below the account's next expected value is a replay of
        // something already committed. Nonces *above* it are accepted only
        // while every nonce in between is pooled (`check_no_gap`); the block
        // executor enforces exact ordering at commit time.
        if tx.nonce < sender.nonce {
            return Err(NodeError::InvalidNonce {
                address: hex::encode(sender_address),
                expected: sender.nonce,
                actual: tx.nonce,
            });
        }
        self.check_no_gap(&sender_address, sender.nonce, tx.nonce)?;

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
        Ok(sender_address)
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
        let miner = self.check_share_nonce(tx)?;
        self.state.check_mining_share(&miner, share)?;
        let height = self.admission_height()?;
        let sunsets = self.state.effective_schedule()?.sunsets;
        let policy = crate::crypto::suites::policy(&self.state.parameters()?, &sunsets)?;
        self.state.verify_cached(tx, height, &policy)?;
        self.check_breaker(&tx.kind)
    }

    /// A share's nonce rules: not stale, and, since a share runs at its exact
    /// nonce like any transaction, the same window and gap rules (CT-04).
    /// Returns the miner.
    fn check_share_nonce(&self, tx: &Transaction) -> Result<Address> {
        let miner = tx.sender();
        let account = self.state.get_account(&miner)?;
        if tx.nonce < account.nonce {
            return Err(NodeError::InvalidNonce {
                address: hex::encode(miner),
                expected: account.nonce,
                actual: tx.nonce,
            });
        }
        self.check_no_gap(&miner, account.nonce, tx.nonce)?;
        Ok(miner)
    }

    /// Whether a pooled entry can still execute, judged by the rules committed
    /// state can change: [`Mempool::check_against_state`], or for a share its
    /// nonce and share rules without the proof of work, then the circuit
    /// breaker. No signature is verified and no work is hashed: both were
    /// checked at admission and are fixed by the entry's bytes, and this runs
    /// on the BFT consensus task every round, where re-verifying a pool of v8
    /// frames or re-hashing shares past the block's share cap would stall it.
    ///
    /// shortcut: a v7 or v8 entry whose suite governance sunsets while it is
    /// pooled is not caught here; the executor drops it and it stays until
    /// evicted. Upgrade if sunsets become frequent: re-check suite
    /// admissibility, which is cheap.
    fn still_executable(&self, tx: &Transaction) -> Result<()> {
        if let crate::core::TxKind::MiningShare(share) = &tx.kind
            && tx.outputs.is_empty()
        {
            let miner = self.check_share_nonce(tx)?;
            self.state.check_mining_share_state(&miner, share)?;
        } else {
            self.check_against_state(tx)?;
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

        // Room first (pre-audit S-016): refusing for a full pool or a sender
        // at its share must not cost a signature check. Re-checked under the
        // write lock below, since another insert may land in between.
        let balance = self.state.get_account(&tx.sender())?.balance;
        let (fee, size) = (fee_of(&tx), tx.to_bytes().len());
        // The decoder refuses a frame this large, but a local submission never
        // is decoded. A transaction over the maximum could never fit a block.
        if size > MAX_TX_BYTES {
            return Err(NodeError::MempoolRejected(format!(
                "transaction of {size} bytes exceeds the maximum {MAX_TX_BYTES}"
            )));
        }
        self.victims(
            self.len(),
            self.pooled_bytes.load(Ordering::Relaxed),
            fee,
            size,
        )?;
        self.check_sender_room(&tx, balance)?;

        // Validation performs RocksDB reads; deliberately outside any lock.
        self.validate(&tx)?;

        let mut pool = self.write_pool();
        if pool.contains_key(&hash) {
            return Ok(false);
        }
        self.check_sender_room(&tx, balance)?;
        // Every mutation holds the write lock, so this read cannot race an add.
        let bytes = self.pooled_bytes.load(Ordering::Relaxed);
        for victim in self.victims(pool.len(), bytes, fee, size)? {
            if let Some(old) = pool.remove(&victim) {
                self.forget(&victim, &old);
            }
        }

        self.track(&tx, true);
        self.lock_fees().insert(hash, (fee, size));
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
        // Admitted transactions passed `total_out`, so this cannot overflow.
        let outputs = total_out(tx).unwrap_or_default();
        let work = share_work(tx);
        let sender = tx.sender();
        let mut senders = self.lock_senders();
        let entry = senders.entry(sender).or_default();
        if added {
            entry.count += 1;
            entry.outputs = entry.outputs.saturating_add(outputs);
            *entry.nonces.entry(tx.nonce).or_default() += 1;
            if let Some(work) = work {
                entry.shares.extend(work);
            }
            return;
        }
        entry.count = entry.count.saturating_sub(1);
        entry.outputs = entry.outputs.saturating_sub(outputs);
        if let Some(uses) = entry.nonces.get_mut(&tx.nonce) {
            *uses = uses.saturating_sub(1);
            if *uses == 0 {
                entry.nonces.remove(&tx.nonce);
            }
        }
        for piece in work.into_iter().flatten() {
            entry.shares.remove(&piece);
        }
        if entry.count == 0 {
            senders.remove(&sender);
        }
    }

    /// A committed round's bookkeeping: removes `included`, then prunes the
    /// `dropped` entries that can never execute. Returns how many dropped
    /// entries were pruned. The driver reports `included` only after the block
    /// committed, so the gap check already starts past those nonces and the
    /// two calls give the same result in either order.
    pub fn settle_round(&self, included: &[TxHash], dropped: &[TxHash]) -> usize {
        self.remove_all(included);
        self.prune_invalid(dropped)
    }

    /// Re-checks only `hashes`, the entries a committed round ordered but
    /// did not execute, and removes those that can never execute: a stale
    /// nonce, a run whose head is gone, a spent share, a balance that no
    /// longer covers them. Returns how many were removed.
    ///
    /// Cost: point reads of committed state per entry, no signature and no
    /// proof of work ([`Mempool::still_executable`]), so it can run on the
    /// consensus task with a full pool.
    ///
    /// Entries are taken in nonce order per sender and removed one at a
    /// time, so removing nonce `n` makes `n + 1` fail the gap check in the
    /// same pass. As in [`Mempool::revalidate`], a state read failure also
    /// removes the entry.
    pub fn prune_invalid(&self, hashes: &[TxHash]) -> usize {
        let mut dropped: Vec<(Address, u64, TxHash, Transaction)> = hashes
            .iter()
            .filter_map(|hash| self.get(hash).map(|tx| (tx.sender(), tx.nonce, *hash, tx)))
            .collect();
        dropped.sort_unstable_by_key(|(sender, nonce, hash, _)| (*sender, *nonce, *hash));
        let mut removed = 0;
        for (_, _, hash, tx) in dropped {
            if self.still_executable(&tx).is_err() {
                removed += self.remove_all(&[hash]);
            }
        }
        removed
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

/// The pieces of work a fee-free mining share batch is credited for, each
/// `(epoch, work nonce)`; empty for a claim. The spent-share key also binds
/// the miner, which is the pool's sender key.
fn share_work(tx: &Transaction) -> Option<Vec<(u64, u64)>> {
    match &tx.kind {
        crate::core::TxKind::MiningShare(share) if is_share(tx) => Some(
            share
                .nonces
                .iter()
                .map(|&nonce| (share.epoch, nonce))
                .collect(),
        ),
        _ => None,
    }
}

/// A fee-free mining share (ADR-044), which moves nothing.
fn is_share(tx: &Transaction) -> bool {
    crate::state::fee_exec::is_free_share(tx)
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
