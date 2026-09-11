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

use crate::config::StorageConfig;
use maya_dex::batch::SwapIntent;
use maya_dex::types::PairId;
use maya_ledger_math as ledger_math;
use rocksdb::{BlockBasedOptions, Cache, DB, IteratorMode, Options, WriteBatch};

use crate::core::payload::ChannelId;
use crate::core::{Block, Transaction};
use crate::error::{NodeError, Result};
use crate::state::account::{Account, Address};
use crate::state::channel::ChannelRecord;
use crate::state::context::BlockContext;
use crate::state::merkle::{HASH_LEN, account_leaf, merkle_path, merkle_root};
use crate::state::proof::{AccountProof, LayerDigest, StateLayer};
use crate::state::shielded::ShieldedPool;
use crate::state::undo::{RecordUndo, UndoEntry, UndoRecord};

/// Key prefix for account records.
pub(crate) const ACCOUNT_PREFIX: &[u8] = b"acct:";

/// Key prefix for per-block undo journals.
pub(crate) const UNDO_PREFIX: &[u8] = b"undo:";

/// Key prefix for channel records.
pub(crate) const CHANNEL_PREFIX: &[u8] = b"chan:";

/// Key prefix for deployed contract code.
pub(crate) const CODE_PREFIX: &[u8] = b"code:";

/// Key prefix for contract storage: `cstate:<contract_id><key>`.
pub(crate) const CSTATE_PREFIX: &[u8] = b"cstate:";

/// Key prefix for spent shielded nullifiers.
///
/// One key per nullifier rather than a single set blob: double-spend detection
/// is then a point lookup, and the set only ever grows.
pub(crate) const NULLIFIER_PREFIX: &[u8] = b"null:";

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
    /// Every record the trading subsystem wrote, keyed by its storage key.
    /// `None` is a deletion.
    ///
    /// One generic map rather than five typed ones — assets, balances, pools,
    /// orders, and the order index. The three commit paths below have to write
    /// identically, and the note above `write_overlay` already records what
    /// happened the last time the number of state kinds grew. Five more maps
    /// would be five more chances for one path to forget one.
    ///
    /// Everything in it lives under the `d:` prefix, so it is also exactly what
    /// the state root and the undo journal need to fold over.
    pub(crate) records: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    /// Swaps staged against each pool, waiting for the end-of-block batch.
    ///
    /// Block-local and never persisted: a swap either settles before the block
    /// commits or is refunded before it commits. There is nothing here for a
    /// later block to find.
    pub(crate) swaps: BTreeMap<PairId, Vec<SwapIntent>>,
    /// Pools whose book or batch this block touched, and which therefore need
    /// an end-of-block pass. Ordered, so the pass visits them identically on
    /// every node.
    pub(crate) touched_pairs: BTreeSet<PairId>,
    /// Order fills produced so far, against the per-block ceiling.
    pub(crate) fills: usize,
    /// The VRF output proved for this block, if a valid proof arrived.
    ///
    /// Staged rather than folded where the transaction sits, because the
    /// accumulator must advance exactly once per block. Folding at the
    /// transaction would make the beacon a function of how many beacon
    /// transactions a miner chose to include.
    pub(crate) beacon_output: Option<Vec<u8>>,
    /// Feed submissions staged so far, against the per-block ceiling.
    ///
    /// Each carries up to a quorum of post-quantum signatures, so this is the
    /// bound on the most expensive verification work a block can demand.
    pub(crate) feed_submissions: usize,
    /// Governance transactions staged so far, against the per-block ceiling.
    pub(crate) governance_actions: usize,
    /// Whether this block has already credited its work to somebody.
    ///
    /// A block represents one unit of work, so it credits one beneficiary.
    /// Admitting a second claim would let a miner mint voting weight out of a
    /// single proof of work.
    pub(crate) work_claimed: bool,
    /// Envelopes this block has accepted, against the per-block ceiling.
    pub(crate) sealed_envelopes: usize,
    /// Decryption shares this block has accepted, against the per-block
    /// ceiling. Each one carries a proof every node verifies.
    pub(crate) reveal_shares: usize,
    /// Envelopes this block opened.
    ///
    /// Counted for the metric and for tests. It is deliberately not a ceiling:
    /// how many envelopes come due at a height was fixed up to
    /// `MAX_REVEAL_DELAY` blocks ago, so refusing to open some of them now
    /// would make the reveal a function of the block rather than of the
    /// commitment. `MAX_SEALED_PER_BLOCK` is where the queue is bounded.
    pub(crate) revealed: usize,
    /// The executing block's difficulty target.
    ///
    /// Staged here rather than added to `BlockContext` because it is needed by
    /// exactly one operation — crediting a work claim — and widening the
    /// context that every state transition takes would put a field in front of
    /// every reader for the sake of one.
    pub(crate) difficulty_target: [u8; HASH_LEN],
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

/// Hashes one stored record into a Merkle leaf.
///
/// Length-prefixed on the key, so a key and value cannot be re-cut into a
/// different pair that hashes the same — the concatenation `ab‖c` and `a‖bc`
/// are one string otherwise, and two different states would share a root.
pub(crate) fn record_leaf(entry: (&Vec<u8>, &Vec<u8>)) -> [u8; HASH_LEN] {
    let (key, value) = entry;
    let mut hasher = blake3::Hasher::new_derive_key("maya aux record leaf v1");
    hasher.update(&(key.len() as u64).to_le_bytes());
    hasher.update(key);
    hasher.update(value);
    *hasher.finalize().as_bytes()
}

pub(crate) fn undo_key(block_id: &[u8; HASH_LEN]) -> Vec<u8> {
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
        Self::open_tuned(path, &StorageConfig::default())
    }

    /// Opens the database with explicit engine tuning.
    ///
    /// # Why this exists
    ///
    /// [`StateDB::open`] used `Options::default()`, which leaves RocksDB's own
    /// defaults in place — and RocksDB's default block cache is 8 MiB. On a
    /// chain-state database that means almost every account read reaches the
    /// disk, and the node's throughput becomes a reading of storage latency
    /// rather than of anything it decided.
    ///
    /// The values come from `config.toml`, because the right ones depend on the
    /// chain's size and the node's traffic — properties of a deployment, not of
    /// the software.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the database cannot be opened.
    pub fn open_tuned<P: AsRef<Path>>(path: P, storage: &StorageConfig) -> Result<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);

        // A shared block cache, sized by configuration. `LruCache` rather than
        // the default: the default is not just small, it is per-column-family,
        // and one shared cache is what makes the configured number mean the
        // total the process will use.
        let mut block_opts = BlockBasedOptions::default();
        let cache = Cache::new_lru_cache(storage.block_cache_mib * 1024 * 1024);
        block_opts.set_block_cache(&cache);
        // Index and filter blocks in the cache too, so the configured figure is
        // the real ceiling rather than the ceiling plus an unbounded amount of
        // metadata.
        block_opts.set_cache_index_and_filter_blocks(true);
        opts.set_block_based_table_factory(&block_opts);

        opts.set_write_buffer_size(storage.write_buffer_mib * 1024 * 1024);
        opts.set_max_open_files(storage.max_open_files);

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
        //
        // The domains and their order live in `state::proof::StateLayer`, not
        // here. A prover and a verifier that each kept their own copy of five
        // string literals would drift, and the drift would look like tampering.
        let mut root = accounts_root;
        for digest in self.state_layers(overlay)? {
            root = digest.layer.fold(&root, &digest.root);
        }

        Ok(root)
    }

    /// Builds a proof that `address` holds what committed state says it holds.
    ///
    /// Returns `None` for an address with no account record. That is not the
    /// same as a zero balance — an absent account and one holding zero are
    /// indistinguishable in this model, and proving *absence* would need the two
    /// neighbouring leaves and an ordering argument this node does not have.
    ///
    /// The proof is built against committed state only. Proving against an
    /// overlay would be proving against a block that has not been accepted, and
    /// a light client cannot check a root no header commits to.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn account_proof(&self, address: &Address) -> Result<Option<AccountProof>> {
        // The whole account set, because a Merkle path is a statement about a
        // leaf's position among all the others. There is no cheaper way to say
        // where something sits in a tree than to know the tree.
        let accounts = self.all_accounts()?;
        let Some(index) = accounts
            .iter()
            .position(|(candidate, _)| candidate == address)
        else {
            return Ok(None);
        };
        let account = accounts[index].1;

        let leaves: Vec<[u8; HASH_LEN]> = accounts
            .iter()
            .map(|(address, account)| account_leaf(address, account))
            .collect();
        let path = merkle_path(&leaves, index).ok_or_else(|| {
            // Unreachable: the index came from the same slice. Reported rather
            // than unwrapped, because a panic here would be a node crashing on
            // an RPC call.
            NodeError::MalformedProof {
                reason: "account index fell outside its own leaf set".to_string(),
            }
        })?;

        Ok(Some(AccountProof {
            address: *address,
            account,
            path,
            layers: self.state_layers(&Overlay::new())?,
        }))
    }

    /// The subsystem layers present in this state, in fold order.
    ///
    /// Only the ones that hold something. An absent layer folds nothing, which
    /// is what keeps a chain that has never traded at the root it would have had
    /// before trading existed — and it is why a proof carries a *list* rather
    /// than a fixed five digests.
    fn state_layers(&self, overlay: &Overlay) -> Result<Vec<LayerDigest>> {
        let mut layers = Vec::new();

        // Escrowed value that no commitment covers is value a light client
        // cannot verify, so channels get folded in under their own domain.
        let mut channels: BTreeMap<ChannelId, ChannelRecord> =
            self.all_channels()?.into_iter().collect();
        for (id, record) in &overlay.channels {
            channels.insert(*id, record.clone());
        }
        if !channels.is_empty() {
            let leaves: Vec<[u8; HASH_LEN]> = channels
                .iter()
                .map(|(id, record)| record.leaf(id))
                .collect();
            layers.push(LayerDigest {
                layer: StateLayer::Channels,
                root: merkle_root(&leaves),
            });
        }

        // Trading, oracle, governance, and the sealed mempool are each one
        // layer over every record under their own prefix, ordered by key so the root is a function of
        // content rather than of the order transactions touched things.
        //
        // The list is `RECORD_LAYERS`, the one source the write-path assertion
        // and the snapshot rules also read, so the three cannot drift.
        for &(prefix, layer) in crate::state::commitments::RECORD_LAYERS {
            let records = self.merged_records(overlay, prefix)?;
            if !records.is_empty() {
                let leaves: Vec<[u8; HASH_LEN]> = records.iter().map(record_leaf).collect();
                layers.push(LayerDigest {
                    layer,
                    root: merkle_root(&leaves),
                });
            }
        }

        // The shielded pool is a second Merkle tree over a different hash —
        // Poseidon, because BLAKE3 is unusable inside a SNARK circuit — so it
        // enters as one opaque commitment rather than as leaves. The commitment
        // covers the whole stored pool, anchors and balance included; see
        // `ShieldedPool::commitment` for what folding the bare root missed.
        //
        // Folded whenever the pool differs from an empty one, not only once it
        // holds a note. Otherwise a stored pool with no notes and an invented
        // anchor would escape the root.
        let pool = self.load_pool(overlay)?;
        let commitment = pool.commitment();
        if commitment != ShieldedPool::new().commitment() {
            layers.push(LayerDigest {
                layer: StateLayer::Shielded,
                root: commitment,
            });
        }

        // Last, after every layer a pre-existing chain could hold, so adding
        // them moved no root that had already been committed. See
        // `state::commitments` for why they were missing.
        layers.extend(self.contracts_layer(overlay)?);
        layers.extend(self.nullifiers_layer(overlay)?);

        Ok(layers)
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
        for (key, value) in &overlay.records {
            // A record under a prefix no layer folds would be state the root
            // does not commit to. Caught here in every debug build and test,
            // at the moment a new subsystem first writes one.
            //
            // Covers the generic `records` map only. The typed fields above
            // build their keys through fixed helpers under committed
            // prefixes; a new typed field is caught by `uncovered_keys` in the
            // tests of whichever subsystem writes it.
            debug_assert!(
                crate::state::commitments::RECORD_LAYERS
                    .iter()
                    .any(|(prefix, _)| key.starts_with(prefix)),
                "record {} is under no committed prefix",
                hex::encode(key)
            );
            match value {
                Some(bytes) => batch.put(key, bytes),
                None => batch.delete(key),
            }
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

    /// Reads a trading record through the overlay, falling back to committed
    /// state.
    ///
    /// A key the overlay maps to `None` reads as absent, not as unwritten: a
    /// cancelled order must not be visible to a later transaction in the same
    /// block simply because the deletion has not reached disk.
    pub(crate) fn record(&self, overlay: &Overlay, key: &[u8]) -> Result<Option<Vec<u8>>> {
        match overlay.records.get(key) {
            Some(staged) => Ok(staged.clone()),
            None => self.raw_get(key),
        }
    }

    /// Stages a trading record.
    pub(crate) fn put_record(overlay: &mut Overlay, key: Vec<u8>, value: Vec<u8>) {
        overlay.records.insert(key, Some(value));
    }

    /// Stages the deletion of a trading record.
    pub(crate) fn delete_record(overlay: &mut Overlay, key: Vec<u8>) {
        overlay.records.insert(key, None);
    }

    /// Committed records under `prefix`, merged with an overlay's changes.
    ///
    /// The overlay's staging map is prefix-agnostic — one generic
    /// `key -> value` area serves every subsystem — so it is the *fold* that
    /// separates them, by scanning one prefix at a time. That is what lets a
    /// new subsystem cost one root layer rather than a parallel overlay map, an
    /// undo section, and three commit paths kept in step by hand.
    pub(crate) fn merged_records(
        &self,
        overlay: &Overlay,
        prefix: &[u8],
    ) -> Result<BTreeMap<Vec<u8>, Vec<u8>>> {
        let mut merged: BTreeMap<Vec<u8>, Vec<u8>> =
            self.scan_prefix(prefix)?.into_iter().collect();
        for (key, value) in &overlay.records {
            if !key.starts_with(prefix) {
                continue;
            }
            match value {
                Some(bytes) => merged.insert(key.clone(), bytes.clone()),
                None => merged.remove(key),
            };
        }
        Ok(merged)
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

        let total_out = ledger_math::total_outputs(tx.outputs.iter().map(|output| output.amount))
            .ok_or(NodeError::BalanceOverflow)?;

        if sender.balance < total_out {
            return Err(NodeError::InsufficientBalance {
                address: hex::encode(sender_address),
                required: total_out,
                available: sender.balance,
            });
        }

        // Debit and advance the nonce. The subtraction cannot underflow given
        // the check above, but stay explicit rather than relying on it.
        sender.balance =
            ledger_math::debit(sender.balance, total_out).ok_or(NodeError::BalanceOverflow)?;
        sender.nonce =
            ledger_math::advance_nonce(sender.nonce).ok_or(NodeError::BalanceOverflow)?;
        overlay.accounts.insert(sender_address, sender);

        // Credit recipients, reading back through the overlay so a self-
        // transfer sees the already-debited balance.
        for output in &tx.outputs {
            let mut recipient = self.load(overlay, &output.recipient)?;
            recipient.balance = ledger_math::credit(recipient.balance, output.amount)
                .ok_or(NodeError::BalanceOverflow)?;
            overlay.accounts.insert(output.recipient, recipient);
        }

        // Typed payloads run last, against an overlay that already reflects the
        // nonce bump and any transfer outputs. A channel operation therefore
        // sees the same account state a following transaction would.
        self.apply_kind(overlay, tx, context)
    }

    /// Stages every transaction in `block`, then runs the end-of-block trading
    /// pass.
    ///
    /// Extracted for the same reason `write_overlay` was: the three commit
    /// paths below have to execute *identically*, and a block whose swaps
    /// cleared on one path and not another would produce two state roots for
    /// one block. That is a fork, not a bug report.
    ///
    /// The pass runs after every transaction has been staged because that is
    /// what makes a batch a batch. A swap settles against the whole block's
    /// flow, not against whatever happened to precede it — see
    /// [`maya_dex::batch`].
    fn stage_block(&self, block: &Block, context: BlockContext) -> Result<Overlay> {
        let mut overlay = Overlay::new();
        // Taken from the header rather than from any transaction, so a work
        // claim credits the difficulty the block was actually mined against
        // and not a number its beneficiary chose.
        overlay.difficulty_target = block.header.difficulty_target;

        for tx in &block.transactions {
            self.stage_transaction(&mut overlay, tx, context)?;
        }

        // Before trading, and after every plaintext transaction. Both halves
        // matter: a revealed swap must reach the same uniform-price batch as
        // the block's plaintext swaps, and a miner who learns a plaintext while
        // assembling this block must not be able to place a transaction ahead
        // of it. See `crate::state::sealed_exec`.
        self.settle_sealed(&mut overlay, context)?;

        self.settle_trading(&mut overlay, context)?;
        // After trading, so a contract or a swap in this block sees the
        // *previous* block's randomness. A beacon folded before execution would
        // be a value the block's own transactions could have been written
        // against, which defeats the point of it being unpredictable.
        self.settle_oracle(&mut overlay, context)?;
        // Last. A rule change applied earlier would mean this block's own
        // transactions executing under two different sets of rules depending on
        // where they sat, which is a state divergence rather than a subtlety.
        self.settle_governance(&mut overlay, context)?;

        Ok(overlay)
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
        let overlay = self.stage_block(block, context)?;

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

        // Trading records go in the same way accounts do: prior value, or
        // absent. Without this a reorg would leave pool reserves at whatever
        // the abandoned chain traded them to, which is the one kind of
        // corruption that looks like ordinary state.
        let mut records = Vec::with_capacity(overlay.records.len());
        for key in overlay.records.keys() {
            records.push(RecordUndo {
                key: key.clone(),
                previous: self.db.get(key).map_err(storage_err)?,
            });
        }

        // Contract code and storage, by the same key/previous rule. They were
        // missing: a reorg that reverted a contract-writing block left the
        // abandoned branch's storage behind, which is invariant 8's failure
        // for contracts. Now that they are under the state root it would also
        // fail the next block's root check and stall the node.
        let contract_keys = overlay.code.keys().map(code_key).chain(
            overlay
                .contract_storage
                .keys()
                .map(|(contract, key)| contract_storage_key(contract, key)),
        );
        for key in contract_keys {
            records.push(RecordUndo {
                previous: self.db.get(&key).map_err(storage_err)?,
                key,
            });
        }

        Ok(UndoRecord {
            entries,
            shielded,
            nullifiers: overlay.nullifiers.iter().copied().collect(),
            records,
        })
    }

    /// The state root `block` would produce, without writing anything.
    ///
    /// What a block producer puts in `header.state_root`: the root *after* the
    /// block executes, at the height it will land at. The chain refuses any
    /// other value, so a candidate built from the pre-block root is a block
    /// nobody accepts.
    ///
    /// # Errors
    ///
    /// Returns the first validation error `block` would hit if applied.
    pub fn preview_root(&self, block: &Block, context: BlockContext) -> Result<[u8; HASH_LEN]> {
        let overlay = self.stage_block(block, context)?;
        self.root_with_overlay(&overlay)
    }

    /// Executes `block`, checks the state root its header declares, commits
    /// it, and records an undo journal under `block_id` so the block can later
    /// be reverted.
    ///
    /// This is the chain's path, so it is the checked one. Before the check
    /// existed a miner could write any `state_root`, and every full node would
    /// accept it — while `light-client` verifies account proofs against exactly
    /// that field.
    ///
    /// The journal and the state changes land in the same [`WriteBatch`]: a
    /// committed block always has a usable undo record, and a rejected one
    /// leaves neither behind.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::StateRootMismatch`] if execution disagrees with the
    /// header, in which case nothing is written. Otherwise as
    /// [`StateDB::apply_block`].
    pub fn apply_block_journaled(
        &self,
        block: &Block,
        block_id: &[u8; HASH_LEN],
        context: BlockContext,
    ) -> Result<[u8; HASH_LEN]> {
        self.apply_journaled_with(block, block_id, context, |_| ())
    }

    /// [`StateDB::apply_block_journaled`], with `extra` writes in the same
    /// batch: how the block store moves the tip and the canonical index
    /// atomically with the state they describe.
    pub(crate) fn apply_journaled_with(
        &self,
        block: &Block,
        block_id: &[u8; HASH_LEN],
        context: BlockContext,
        extra: impl FnOnce(&mut WriteBatch),
    ) -> Result<[u8; HASH_LEN]> {
        let (overlay, new_root) = self.stage_checked(block, context)?;
        let undo = self.capture_undo(&overlay)?;

        let mut batch = WriteBatch::default();
        self.write_overlay(&mut batch, &overlay);
        batch.put(undo_key(block_id), undo.encode());
        extra(&mut batch);
        self.db.write(batch).map_err(storage_err)?;

        Ok(new_root)
    }

    /// Writes a batch built elsewhere in the crate.
    pub(crate) fn write_batch(&self, batch: WriteBatch) -> Result<()> {
        self.db.write(batch).map_err(storage_err)
    }

    /// Writes a RocksDB checkpoint of the whole database to `path`: hard
    /// links where the filesystem allows, so it is cheap and immediate, and a
    /// consistent point-in-time copy.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the checkpoint cannot be made, for
    /// example because `path` already exists.
    pub fn create_checkpoint(&self, path: impl AsRef<Path>) -> Result<()> {
        rocksdb::checkpoint::Checkpoint::new(&self.db)
            .and_then(|checkpoint| checkpoint.create_checkpoint(path))
            .map_err(storage_err)
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
        self.revert_block_with(block_id, |_| ())
    }

    /// [`StateDB::revert_block`], with `extra` writes in the same batch.
    pub(crate) fn revert_block_with(
        &self,
        block_id: &[u8; HASH_LEN],
        extra: impl FnOnce(&mut WriteBatch),
    ) -> Result<()> {
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

        for record in &undo.records {
            match &record.previous {
                Some(bytes) => batch.put(&record.key, bytes),
                None => batch.delete(&record.key),
            }
        }

        batch.delete(&key);
        extra(&mut batch);
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
        let (overlay, new_root) = self.stage_checked(block, context)?;

        let mut batch = WriteBatch::default();
        self.write_overlay(&mut batch, &overlay);
        self.db.write(batch).map_err(storage_err)?;

        Ok(new_root)
    }

    /// Stages `block` and checks the state root its header declares, writing
    /// nothing. The one place the comparison lives, shared by both checked
    /// apply paths so they cannot drift apart.
    fn stage_checked(
        &self,
        block: &Block,
        context: BlockContext,
    ) -> Result<(Overlay, [u8; HASH_LEN])> {
        let overlay = self.stage_block(block, context)?;
        let new_root = self.root_with_overlay(&overlay)?;
        if new_root != block.header.state_root {
            return Err(NodeError::StateRootMismatch {
                expected: hex::encode(block.header.state_root),
                actual: hex::encode(new_root),
            });
        }
        Ok((overlay, new_root))
    }
}
