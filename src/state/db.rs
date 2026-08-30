//! Persistent account state backed by RocksDB.
//!
//! ## Atomicity model
//!
//! Block execution never writes to RocksDB incrementally. Every mutation lands
//! first in an in-memory overlay; only after *all* transactions in the block
//! have passed validation is that overlay flushed as a single
//! [`rocksdb::WriteBatch`]. A failure at any transaction returns `Err` before
//! the batch is ever handed to the database, so a rejected block leaves state
//! byte-for-byte unchanged. There is no partial application and nothing to undo.
//!
//! The overlay is also what makes intra-block double-spend detection work: each
//! transaction reads the effects of its predecessors in the same block, not the
//! stale on-disk values.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rocksdb::{DB, IteratorMode, Options, WriteBatch};

use crate::core::payload::ChannelId;
use crate::core::{Block, Transaction};
use crate::error::{NodeError, Result};
use crate::state::account::{Account, Address};
use crate::state::channel::ChannelRecord;
use crate::state::context::BlockContext;
use crate::state::merkle::{HASH_LEN, account_leaf, merkle_root};
use crate::state::shielded::ShieldedPool;
use crate::state::undo::{UndoEntry, UndoRecord};

/// Key prefix for account records.
const ACCOUNT_PREFIX: &[u8] = b"acct:";

/// Key prefix for per-block undo journals.
const UNDO_PREFIX: &[u8] = b"undo:";

/// Key prefix for channel records.
const CHANNEL_PREFIX: &[u8] = b"chan:";

/// Key prefix for deployed contract code.
const CODE_PREFIX: &[u8] = b"code:";

/// Key prefix for contract storage: `cstate:<contract_id><key>`.
const CSTATE_PREFIX: &[u8] = b"cstate:";

/// Key prefix for spent shielded nullifiers.
///
/// One key per nullifier rather than a single set blob: double-spend detection
/// is then a point lookup, and the set only ever grows.
const NULLIFIER_PREFIX: &[u8] = b"null:";

/// Key holding the serialized shielded pool.
pub(crate) const POOL_KEY: &[u8] = b"shld:pool";

/// Staged mutations for one block.
///
/// Both maps are ordered so the Merkle root is a function of content rather
/// than of the order transactions happened to touch things.
#[derive(Debug, Default)]
pub(crate) struct Overlay {
    /// Accounts written by this block.
    pub(crate) accounts: BTreeMap<Address, Account>,
    /// Channel records created or modified by this block.
    pub(crate) channels: BTreeMap<ChannelId, ChannelRecord>,
    /// Contract code deployed by this block.
    pub(crate) code: BTreeMap<ChannelId, Vec<u8>>,
    /// Contract storage written by this block, keyed by contract then key.
    pub(crate) contract_storage: BTreeMap<(ChannelId, Vec<u8>), Vec<u8>>,
    /// Nullifiers spent by this block.
    ///
    /// Checked alongside the committed set, so two joinsplits in one block
    /// cannot spend the same note — the on-disk set would not show the first
    /// spend until after the batch lands.
    pub(crate) nullifiers: BTreeSet<[u8; HASH_LEN]>,
    /// Working copy of the shielded pool, loaded on the block's first
    /// joinsplit and left untouched by blocks that have none.
    pub(crate) shielded: Option<ShieldedPool>,
    /// Joinsplits staged so far, for the per-block verification budget.
    pub(crate) joinsplits: usize,
}

impl Overlay {
    fn new() -> Self {
        Self::default()
    }
}

fn storage_err(e: rocksdb::Error) -> NodeError {
    NodeError::Storage(e.to_string())
}

fn account_key(address: &Address) -> Vec<u8> {
    let mut key = Vec::with_capacity(ACCOUNT_PREFIX.len() + address.len());
    key.extend_from_slice(ACCOUNT_PREFIX);
    key.extend_from_slice(address);
    key
}

pub(crate) fn code_key(contract: &ChannelId) -> Vec<u8> {
    let mut key = Vec::with_capacity(CODE_PREFIX.len() + contract.len());
    key.extend_from_slice(CODE_PREFIX);
    key.extend_from_slice(contract);
    key
}

pub(crate) fn contract_storage_key(contract: &ChannelId, storage_key: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(CSTATE_PREFIX.len() + contract.len() + storage_key.len());
    key.extend_from_slice(CSTATE_PREFIX);
    key.extend_from_slice(contract);
    key.extend_from_slice(storage_key);
    key
}

/// Prefix under which one contract's storage lives.
pub(crate) fn contract_storage_prefix(contract: &ChannelId) -> Vec<u8> {
    let mut key = Vec::with_capacity(CSTATE_PREFIX.len() + contract.len());
    key.extend_from_slice(CSTATE_PREFIX);
    key.extend_from_slice(contract);
    key
}

pub(crate) fn channel_key(channel_id: &ChannelId) -> Vec<u8> {
    let mut key = Vec::with_capacity(CHANNEL_PREFIX.len() + channel_id.len());
    key.extend_from_slice(CHANNEL_PREFIX);
    key.extend_from_slice(channel_id);
    key
}

pub(crate) fn nullifier_key_bytes(nullifier: &[u8; HASH_LEN]) -> Vec<u8> {
    let mut key = Vec::with_capacity(NULLIFIER_PREFIX.len() + nullifier.len());
    key.extend_from_slice(NULLIFIER_PREFIX);
    key.extend_from_slice(nullifier);
    key
}

fn undo_key(block_id: &[u8; HASH_LEN]) -> Vec<u8> {
    let mut key = Vec::with_capacity(UNDO_PREFIX.len() + block_id.len());
    key.extend_from_slice(UNDO_PREFIX);
    key.extend_from_slice(block_id);
    key
}

/// Persistent account state.
pub struct StateDB {
    db: DB,
}

impl StateDB {
    /// Opens (or creates) a state database at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the database cannot be opened.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = DB::open(&opts, path).map_err(storage_err)?;
        Ok(Self { db })
    }

    /// Reads an account, returning [`Account::default`] for an unknown address.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure, or
    /// [`NodeError::InvalidLength`] if the stored record is corrupt.
    pub fn get_account(&self, address: &Address) -> Result<Account> {
        match self.db.get(account_key(address)).map_err(storage_err)? {
            Some(bytes) => Account::decode(&bytes),
            None => Ok(Account::default()),
        }
    }

    /// Writes a single account. Intended for genesis allocation and tests;
    /// block execution goes through [`StateDB::apply_block`].
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a write failure.
    pub fn put_account(&self, address: &Address, account: &Account) -> Result<()> {
        self.db
            .put(account_key(address), account.encode())
            .map_err(storage_err)
    }

    /// Raw read by key. Internal plumbing for sibling state modules.
    pub(crate) fn raw_get(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.db.get(key).map_err(storage_err)
    }

    /// Raw write by key. Internal plumbing for sibling state modules.
    pub(crate) fn raw_put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        self.db.put(key, value).map_err(storage_err)
    }

    /// Every key/value pair under `prefix`, in key order.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on an iteration failure.
    pub(crate) fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mode = IteratorMode::From(prefix, rocksdb::Direction::Forward);
        let mut out = Vec::new();
        for item in self.db.iterator(mode) {
            let (key, value) = item.map_err(storage_err)?;
            if !key.starts_with(prefix) {
                break;
            }
            out.push((key.to_vec(), value.to_vec()));
        }
        Ok(out)
    }

    /// Reads every channel record in identifier order.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on an iteration failure.
    pub fn all_channels(&self) -> Result<Vec<(ChannelId, ChannelRecord)>> {
        let mode = IteratorMode::From(CHANNEL_PREFIX, rocksdb::Direction::Forward);
        let mut channels = Vec::new();

        for item in self.db.iterator(mode) {
            let (key, value) = item.map_err(storage_err)?;
            if !key.starts_with(CHANNEL_PREFIX) {
                break;
            }
            let raw = &key[CHANNEL_PREFIX.len()..];
            let Ok(id) = <ChannelId>::try_from(raw) else {
                return Err(NodeError::InvalidLength {
                    what: "channel key",
                    expected: CHANNEL_PREFIX.len() + 32,
                    actual: key.len(),
                });
            };
            channels.push((id, ChannelRecord::decode(&value)?));
        }

        Ok(channels)
    }

    /// Reads every account in address order.
    ///
    /// RocksDB iterates lexicographically, and the shared prefix preserves that
    /// order over raw addresses, so no explicit sort is needed.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on an iteration failure, or
    /// [`NodeError::InvalidLength`] if a stored record is corrupt.
    pub fn all_accounts(&self) -> Result<Vec<(Address, Account)>> {
        let mode = IteratorMode::From(ACCOUNT_PREFIX, rocksdb::Direction::Forward);
        let mut accounts = Vec::new();

        for item in self.db.iterator(mode) {
            let (key, value) = item.map_err(storage_err)?;

            // Seeking positions the cursor at the prefix; stop once past it.
            if !key.starts_with(ACCOUNT_PREFIX) {
                break;
            }

            let raw = &key[ACCOUNT_PREFIX.len()..];
            let Ok(address) = <Address>::try_from(raw) else {
                return Err(NodeError::InvalidLength {
                    what: "account key",
                    expected: ACCOUNT_PREFIX.len() + 32,
                    actual: key.len(),
                });
            };

            accounts.push((address, Account::decode(&value)?));
        }

        Ok(accounts)
    }

    /// Computes the Merkle state root of the committed state.
    ///
    /// # Errors
    ///
    /// Propagates read failures from [`StateDB::all_accounts`].
    pub fn state_root(&self) -> Result<[u8; HASH_LEN]> {
        self.root_with_overlay(&Overlay::new())
    }

    /// Computes the state root that *would* result from applying `overlay`,
    /// without writing anything.
    ///
    /// This is what allows a block's committed state root to be checked before
    /// the batch is durable.
    fn root_with_overlay(&self, overlay: &Overlay) -> Result<[u8; HASH_LEN]> {
        let mut merged: BTreeMap<Address, Account> = self.all_accounts()?.into_iter().collect();
        for (address, account) in &overlay.accounts {
            merged.insert(*address, *account);
        }

        let leaves: Vec<[u8; HASH_LEN]> = merged
            .iter()
            .map(|(address, account)| account_leaf(address, account))
            .collect();
        let accounts_root = merkle_root(&leaves);

        let mut channels: BTreeMap<ChannelId, ChannelRecord> =
            self.all_channels()?.into_iter().collect();
        for (id, record) in &overlay.channels {
            channels.insert(*id, record.clone());
        }

        // Each layer folds in only once it holds anything, so every state root
        // a pre-existing chain committed to stays exactly what it was. With no
        // channels and no notes, the root is still just the accounts root.
        let mut root = accounts_root;

        // Escrowed value that no commitment covers is value a light client
        // cannot verify, so channels get folded in under their own domain.
        if !channels.is_empty() {
            let channel_leaves: Vec<[u8; HASH_LEN]> = channels
                .iter()
                .map(|(id, record)| record.leaf(id))
                .collect();
            let channels_root = merkle_root(&channel_leaves);

            let mut hasher = blake3::Hasher::new_derive_key("maya-flash state root v1");
            hasher.update(&root);
            hasher.update(&channels_root);
            root = *hasher.finalize().as_bytes();
        }

        // The shielded pool is a second Merkle tree over a different hash —
        // Poseidon, because BLAKE3 is unusable inside a SNARK circuit — so it
        // enters the state root as one opaque commitment rather than as leaves.
        let pool = self.load_pool(overlay)?;
        if pool.note_count() > 0 {
            let mut hasher = blake3::Hasher::new_derive_key("maya shielded state root v1");
            hasher.update(&root);
            hasher.update(&pool.root());
            root = *hasher.finalize().as_bytes();
        }

        Ok(root)
    }

    /// Stages every overlay mutation into `batch`.
    ///
    /// Extracted because the three commit paths must write *identically*; a
    /// fifth and sixth kind of state made keeping three copies in step a matter
    /// of time rather than care.
    fn write_overlay(&self, batch: &mut WriteBatch, overlay: &Overlay) {
        for (address, account) in &overlay.accounts {
            batch.put(account_key(address), account.encode());
        }
        for (id, record) in &overlay.channels {
            batch.put(channel_key(id), record.encode());
        }
        for (id, code) in &overlay.code {
            batch.put(code_key(id), code);
        }
        for ((contract, key), value) in &overlay.contract_storage {
            batch.put(contract_storage_key(contract, key), value);
        }
        for nullifier in &overlay.nullifiers {
            // The value is irrelevant; presence of the key is the whole record.
            batch.put(nullifier_key_bytes(nullifier), []);
        }
        if let Some(pool) = &overlay.shielded {
            // Sealing here rather than at append time means one anchor per
            // block, not one per joinsplit, so the window spans blocks rather
            // than being consumed by a single busy one.
            let mut sealed = pool.clone();
            sealed.seal_anchor();
            batch.put(POOL_KEY, sealed.encode());
        }
    }

    /// Reads an account through the overlay, falling back to committed state.
    fn load(&self, overlay: &Overlay, address: &Address) -> Result<Account> {
        match overlay.accounts.get(address) {
            Some(account) => Ok(*account),
            None => self.get_account(address),
        }
    }

    /// Validates and stages one transaction against `overlay`.
    ///
    /// Order matters: the sender is debited and re-staged *before* recipients
    /// are credited, so a self-transfer nets to zero instead of minting value.
    fn stage_transaction(
        &self,
        overlay: &mut Overlay,
        tx: &Transaction,
        context: BlockContext,
    ) -> Result<()> {
        // Authorization first — an unsigned or forged transaction must never
        // reach the balance rules.
        //
        // This one call is the chain's post-quantum rule. `Transaction::verify`
        // passes only when *both* the ML-DSA-65 (FIPS 204) and the
        // SLH-DSA-SHA2-128s (FIPS 205) proofs check out, so a transaction that
        // satisfies one scheme and not the other is rejected here, before it
        // can move a single unit of value. There is deliberately no branch, no
        // configuration flag, and no legacy path that would accept one proof:
        // an adversary who broke either scheme in isolation gets nothing.
        tx.verify()?;

        // Derived from the keys the signatures were just checked against, never
        // read from a wire field. A transaction able to name a sender
        // independently of the keys that signed it would authorize spending
        // from an account it does not control. The address hashes both keys, so
        // a forged half paired with an attacker-chosen partner key resolves to
        // an account that holds nothing.
        let sender_address = tx.sender();
        let mut sender = self.load(overlay, &sender_address)?;

        // Nonce check. This is the double-spend guard: the first spend bumps
        // the nonce, so a replay of the same transaction no longer matches.
        if tx.nonce != sender.nonce {
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

        // Debit and advance the nonce. The subtraction cannot underflow given
        // the check above, but stay explicit rather than relying on it.
        sender.balance = sender
            .balance
            .checked_sub(total_out)
            .ok_or(NodeError::BalanceOverflow)?;
        sender.nonce = sender
            .nonce
            .checked_add(1)
            .ok_or(NodeError::BalanceOverflow)?;
        overlay.accounts.insert(sender_address, sender);

        // Credit recipients, reading back through the overlay so a self-
        // transfer sees the already-debited balance.
        for output in &tx.outputs {
            let mut recipient = self.load(overlay, &output.recipient)?;
            recipient.balance = recipient
                .balance
                .checked_add(output.amount)
                .ok_or(NodeError::BalanceOverflow)?;
            overlay.accounts.insert(output.recipient, recipient);
        }

        // Typed payloads run last, against an overlay that already reflects the
        // nonce bump and any transfer outputs. A channel operation therefore
        // sees the same account state a following transaction would.
        self.apply_kind(overlay, tx, context)
    }

    /// Executes `block` and commits it atomically.
    ///
    /// Returns the new state root on success. On any failure — bad signature,
    /// wrong nonce, insufficient funds — nothing is written and committed state
    /// is unchanged.
    ///
    /// # Errors
    ///
    /// Returns the first validation error encountered, or
    /// [`NodeError::Storage`] if the final batch write fails.
    pub fn apply_block(&self, block: &Block, context: BlockContext) -> Result<[u8; HASH_LEN]> {
        let mut overlay = Overlay::new();

        for tx in &block.transactions {
            self.stage_transaction(&mut overlay, tx, context)?;
        }

        let new_root = self.root_with_overlay(&overlay)?;

        // Single atomic write. Until this line runs, the database has not been
        // touched by this block at all.
        let mut batch = WriteBatch::default();
        self.write_overlay(&mut batch, &overlay);
        self.db.write(batch).map_err(storage_err)?;

        Ok(new_root)
    }

    /// Captures the prior value of every account an overlay would overwrite.
    fn capture_undo(&self, overlay: &Overlay) -> Result<UndoRecord> {
        let mut entries = Vec::with_capacity(overlay.accounts.len());
        for address in overlay.accounts.keys() {
            // Distinguish "absent" from "present with zero balance": restoring
            // the wrong one changes the Merkle root after a revert.
            let previous = match self.db.get(account_key(address)).map_err(storage_err)? {
                Some(bytes) => Some(Account::decode(&bytes)?),
                None => None,
            };
            entries.push(UndoEntry {
                address: *address,
                previous,
            });
        }

        // Only blocks that touched the pool need to record it, so an ordinary
        // block's journal is exactly what it was before shielded transactions
        // existed.
        let shielded = if overlay.shielded.is_some() {
            Some(self.stored_pool()?.encode())
        } else {
            None
        };

        Ok(UndoRecord {
            entries,
            shielded,
            nullifiers: overlay.nullifiers.iter().copied().collect(),
        })
    }

    /// Executes `block`, commits it, and records an undo journal under
    /// `block_id` so the block can later be reverted.
    ///
    /// The journal and the state changes land in the same [`WriteBatch`]: a
    /// committed block always has a usable undo record, and a rejected one
    /// leaves neither behind.
    ///
    /// # Errors
    ///
    /// As [`StateDB::apply_block`].
    pub fn apply_block_journaled(
        &self,
        block: &Block,
        block_id: &[u8; HASH_LEN],
        context: BlockContext,
    ) -> Result<[u8; HASH_LEN]> {
        let mut overlay = Overlay::new();
        for tx in &block.transactions {
            self.stage_transaction(&mut overlay, tx, context)?;
        }

        let new_root = self.root_with_overlay(&overlay)?;
        let undo = self.capture_undo(&overlay)?;

        let mut batch = WriteBatch::default();
        self.write_overlay(&mut batch, &overlay);
        batch.put(undo_key(block_id), undo.encode());
        self.db.write(batch).map_err(storage_err)?;

        Ok(new_root)
    }

    /// Reverses a previously journaled block.
    ///
    /// Restores every touched account to its prior value, deletes accounts that
    /// did not exist before the block, and drops the journal — atomically.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if no journal exists for `block_id`,
    /// which means the block was never applied through
    /// [`StateDB::apply_block_journaled`] or has already been reverted.
    pub fn revert_block(&self, block_id: &[u8; HASH_LEN]) -> Result<()> {
        let key = undo_key(block_id);
        let encoded = self.db.get(&key).map_err(storage_err)?.ok_or_else(|| {
            NodeError::Storage(format!(
                "no undo journal for block {}",
                hex::encode(block_id)
            ))
        })?;

        let undo = UndoRecord::decode(&encoded)?;

        let mut batch = WriteBatch::default();
        for entry in &undo.entries {
            match &entry.previous {
                Some(account) => batch.put(account_key(&entry.address), account.encode()),
                None => batch.delete(account_key(&entry.address)),
            }
        }

        // Restoring the frontier rewinds the commitment tree; deleting the
        // nullifiers makes those notes spendable again, which is correct
        // because on the chain we are reverting to they were never spent.
        if let Some(pool) = &undo.shielded {
            batch.put(POOL_KEY, pool);
        }
        for nullifier in &undo.nullifiers {
            batch.delete(nullifier_key_bytes(nullifier));
        }

        batch.delete(&key);
        self.db.write(batch).map_err(storage_err)?;

        Ok(())
    }

    /// Whether an undo journal exists for `block_id`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn has_undo(&self, block_id: &[u8; HASH_LEN]) -> Result<bool> {
        Ok(self
            .db
            .get(undo_key(block_id))
            .map_err(storage_err)?
            .is_some())
    }

    /// Executes `block` and commits it only if the resulting state root matches
    /// the root committed in the block header.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::StateRootMismatch`] if execution disagrees with the
    /// header, in which case nothing is written. Otherwise propagates the same
    /// errors as [`StateDB::apply_block`].
    pub fn apply_block_checked(
        &self,
        block: &Block,
        context: BlockContext,
    ) -> Result<[u8; HASH_LEN]> {
        let mut overlay = Overlay::new();

        for tx in &block.transactions {
            self.stage_transaction(&mut overlay, tx, context)?;
        }

        let new_root = self.root_with_overlay(&overlay)?;
        if new_root != block.header.state_root {
            return Err(NodeError::StateRootMismatch {
                expected: hex::encode(block.header.state_root),
                actual: hex::encode(new_root),
            });
        }

        let mut batch = WriteBatch::default();
        self.write_overlay(&mut batch, &overlay);
        self.db.write(batch).map_err(storage_err)?;

        Ok(new_root)
    }
}
