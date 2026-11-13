//! The incremental sparse accounts root (ADR-062a).
//!
//! ## Why
//!
//! `sparse_accounts_root` reads every account and hashes the whole tree, every
//! block. That is O(all accounts) for the producer and every validator, and an
//! attacker grows "all accounts" for the price of dust outputs
//! (`reports/sessions/2026-11-13-per-block-scan-bounds.md`).
//!
//! ## Scope
//!
//! The accounts layer only, and only once the sparse tree is active
//! (`FeatureSet::STATELESS`; `STATELESS_ACTIVATION_HEIGHT = u64::MAX`). While
//! accounts are the dense tree no cache is kept, and the record layers are
//! full rebuilds either way.
//!
//! ## What is cached
//!
//! One entry per *branch*: a node of `maya-stateless-core`'s sparse tree with
//! accounts on both sides. Key: `smt:` ‖ the node's key bits, zero-padded to
//! 32 bytes ‖ its depth. Value: the digests of its two children as it sees
//! them. A tree of N accounts has exactly N − 1 branches, whatever the
//! addresses.
//!
//! Single-child levels are not stored. `sparse::subtree` hashes each one as
//! `internal(child, empty)`, and the walk folds such a run in memory, at most
//! 256 hashes and no read. Storing those levels let two addresses that differ
//! only in their last bit cost about 240 entries each (PR #540 review, HIGH).
//!
//! Key order puts a node before everything below it (same padded bits,
//! smaller depth; or smaller bits), so the topmost branch under a prefix is
//! one seek. An absent branch means at most one account below, which is one
//! seek into the `acct:` range the prefix spells.
//!
//! ## What a block costs
//!
//! Per touched key: one seek per branch on its path, and at most 2 × 256
//! in-memory hashes. Untouched siblings cost nothing: their digests are in
//! their parent's entry. Branches on a path number about log2(N) for
//! addresses nobody chose, and up to 256 for addresses an attacker lines up
//! along one path, each such branch costing the attacker an account.
//!
//! A block that touches enough keys is cheaper as a from-scratch build over
//! every account (`worth_walking`). Both give the same root.
//!
//! ## When it is trusted
//!
//! The bare key `smt:` holds the committed root the entries describe and the
//! number of accounts below it. It is present only while the cache matches
//! committed state:
//!
//! - every commit path stages the cache's writes in the same `WriteBatch` as
//!   the accounts they follow, so a crash leaves both or neither
//!   (invariant 26);
//! - every other write that can touch `acct:` (`put_account`, `raw_put`,
//!   `write_batch`, which also treats an operation it cannot inspect as one)
//!   deletes it, and the next commit rebuilds the cache;
//! - a commit whose post-state is not sparse deletes it;
//! - `verify_root_cache`, run when a chain opens, compares it with a full
//!   rebuild and rebuilds on any difference;
//! - a block whose declared root matches the full rebuild but not the cache
//!   is accepted on the full rebuild, and the cache is withdrawn
//!   (`StateDB::check_declared_root`).
//!
//! It is not state: two nodes with different caches compute the same root, and
//! `smt:` is on `LOCAL_ONLY_PREFIXES` (invariant 25), so no snapshot carries it.
//! The full rebuild (`StateDB::state_root`) stays the reference, and the root
//! value is the full rebuild's, bit for bit.

use std::sync::atomic::{AtomicU64, Ordering};

use maya_stateless_core::params::KEY_BITS;
use maya_stateless_core::{Key, Value};
use rocksdb::{DB, IteratorMode, SnapshotWithThreadMode, WriteBatch};

use crate::error::{NodeError, Result};
use crate::state::account::ACCOUNT_LEN;
use crate::state::db::ACCOUNT_PREFIX;
use crate::state::merkle::HASH_LEN;

/// Prefix of the cache. Local-only (invariant 25).
pub(crate) const ROOT_CACHE_PREFIX: &[u8] = b"smt:";

/// The marker: present iff the cache describes committed state. Node keys are
/// longer, so it is unique.
const MARKER: &[u8] = ROOT_CACHE_PREFIX;

/// First key after every node key (`;` follows `:`), for `delete_range`.
const NODES_END: &[u8] = b"smt;";

const PREFIX_LEN: usize = ROOT_CACHE_PREFIX.len();
const KEY_BYTES: usize = KEY_BITS / 8;
const NODE_KEY_LEN: usize = PREFIX_LEN + KEY_BYTES + 1;
/// The marker's value: the root, then the account count, little-endian.
const MARKER_LEN: usize = HASH_LEN + 8;

/// One seek's cost in accounts of a full rebuild. `root_cost_by_state_size`
/// (test profile) measured 1.3-1.4 µs a seek against 1.06-1.09 µs an account
/// at 100k and 1M accounts. Rounded up to 2: the rebuild's cost does not
/// depend on which addresses a block touches, the walk's does.
const SEEK_COST_IN_ACCOUNTS: u64 = 2;

/// Below this many accounts a block always walks: either path then costs a
/// few milliseconds, and walking keeps the incremental path exercised on small
/// chains and in every test that runs one.
const ALWAYS_WALK_BELOW: u64 = 1 << 12;

type Digest = [u8; HASH_LEN];
type NodeKey = [u8; NODE_KEY_LEN];
/// A stored branch: its left then right child's digest, as it sees them.
type Children = [u8; 2 * HASH_LEN];

/// One account change: `None` deletes the account (a revert).
pub(crate) type Change = (Key, Option<Value>);

/// A cache write: `None` deletes the branch.
type Write = (NodeKey, Option<Children>);

mod walk;

use walk::{Walk, WalkError, WalkResult, node_key, storage_err, to_u64};

/// The root and account count the cache claims in `snapshot`, or `None` when
/// it claims nothing.
fn read_marker(snapshot: &SnapshotWithThreadMode<'_, DB>) -> Result<Option<(Digest, u64)>> {
    let Some(bytes) = snapshot.get(MARKER).map_err(|e| storage_err(&e))? else {
        return Ok(None);
    };
    let Ok(bytes) = <[u8; MARKER_LEN]>::try_from(bytes.as_slice()) else {
        return Ok(None);
    };
    let mut root = [0; HASH_LEN];
    root.copy_from_slice(&bytes[..HASH_LEN]);
    let mut count = [0; 8];
    count.copy_from_slice(&bytes[HASH_LEN..]);
    Ok(Some((root, u64::from_le_bytes(count))))
}

fn marker_value(root: &Digest, accounts: u64) -> [u8; MARKER_LEN] {
    let mut value = [0; MARKER_LEN];
    value[..HASH_LEN].copy_from_slice(root);
    value[HASH_LEN..].copy_from_slice(&accounts.to_le_bytes());
    value
}

/// The committed root and account count the cache claims, or `None`.
pub(crate) fn marker(db: &DB) -> Result<Option<(Digest, u64)>> {
    read_marker(&db.snapshot())
}

/// Every committed account in `snapshot`, in address order.
fn snapshot_leaves(snapshot: &SnapshotWithThreadMode<'_, DB>) -> Result<Vec<(Key, Value)>> {
    let mut leaves = Vec::new();
    let mode = IteratorMode::From(ACCOUNT_PREFIX, rocksdb::Direction::Forward);
    for item in snapshot.iterator(mode) {
        let (key, value) = item.map_err(|e| storage_err(&e))?;
        let Some(raw) = key.strip_prefix(ACCOUNT_PREFIX) else {
            break;
        };
        let malformed =
            || NodeError::Storage(format!("malformed account record {}", hex::encode(&key)));
        let address = Key::try_from(raw).map_err(|_| malformed())?;
        let value = <[u8; ACCOUNT_LEN]>::try_from(&*value).map_err(|_| malformed())?;
        leaves.push((address, value));
    }
    Ok(leaves)
}

/// Whether walking `touched` keys beats rebuilding over `accounts`.
fn worth_walking(touched: usize, accounts: u64) -> bool {
    let depth = u64::from(u64::BITS - accounts.leading_zeros()) + 1;
    accounts < ALWAYS_WALK_BELOW
        || to_u64(touched).saturating_mul(depth * SEEK_COST_IN_ACCOUNTS) <= accounts
}

/// What the cache did, counted per database, so an operator sees it going
/// wrong: `repaired` rises only when a cache was found disagreeing with the
/// accounts it described.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RootCacheStats {
    /// Roots computed from the cache, rehashing touched paths only.
    pub walked: u64,
    /// Roots computed by a build over every account: no cache yet, a block
    /// too large to walk, or a repair.
    pub built: u64,
    /// Caches found wrong and rebuilt or withdrawn.
    pub repaired: u64,
}

/// [`RootCacheStats`], as the database keeps it.
#[derive(Debug, Default)]
pub(crate) struct Counters {
    walked: AtomicU64,
    built: AtomicU64,
    repaired: AtomicU64,
}

impl Counters {
    pub(crate) fn stats(&self) -> RootCacheStats {
        RootCacheStats {
            walked: self.walked.load(Ordering::Relaxed),
            built: self.built.load(Ordering::Relaxed),
            repaired: self.repaired.load(Ordering::Relaxed),
        }
    }

    /// Records, and says on stderr (the node's log), that a cache was wrong.
    pub(crate) fn note_repair(&self, what: &str) {
        let count = self.repaired.fetch_add(1, Ordering::Relaxed) + 1;
        eprintln!("state: sparse root cache {what}; rebuilding it (repair #{count}, ADR-062a)");
    }
}

/// The sparse accounts root after a block's account changes and the cache
/// writes that follow it, computed once with the root and staged at commit if
/// committed state has not moved since.
#[derive(Clone, Debug)]
pub(crate) struct Plan {
    /// The write generation read before the snapshot the plan came from.
    generation: u64,
    changes: Vec<Change>,
    root: Digest,
    accounts: u64,
    /// From the cache (writes are a delta), not a build over every account
    /// (writes replace the cache).
    walked: bool,
    delta: Vec<Write>,
}

impl Plan {
    pub(crate) const fn root(&self) -> Digest {
        self.root
    }

    pub(crate) const fn walked(&self) -> bool {
        self.walked
    }

    /// Whether this plan still describes a commit of `changes` at
    /// `generation`.
    pub(crate) fn fits(&self, generation: u64, changes: &[Change]) -> bool {
        self.generation == generation && self.changes == changes
    }
}

/// Plans the root after `changes` (sorted, distinct) from the cache when it is
/// valid and the block small enough, else from every account. `generation` is
/// the write generation read before this call.
pub(crate) fn plan(
    db: &DB,
    generation: u64,
    changes: Vec<Change>,
    counters: &Counters,
) -> Result<Plan> {
    let snapshot = db.snapshot();
    if let Some((root, accounts)) = read_marker(&snapshot)?
        && worth_walking(changes.len(), accounts)
    {
        match walk(&snapshot, &changes, root, accounts) {
            Ok((root, accounts, delta)) => {
                counters.walked.fetch_add(1, Ordering::Relaxed);
                return Ok(Plan {
                    generation,
                    changes,
                    root,
                    accounts,
                    walked: true,
                    delta,
                });
            }
            Err(WalkError::Node(error)) => return Err(error),
            Err(WalkError::Stale) => counters.note_repair("disagrees with the accounts"),
        }
    }
    counters.built.fetch_add(1, Ordering::Relaxed);
    build_plan(&snapshot, generation, changes)
}

/// A from-scratch plan over every account, whatever the cache holds.
pub(crate) fn rebuild(db: &DB, generation: u64, counters: &Counters) -> Result<Plan> {
    counters.built.fetch_add(1, Ordering::Relaxed);
    build_plan(&db.snapshot(), generation, Vec::new())
}

fn walk(
    snapshot: &SnapshotWithThreadMode<'_, DB>,
    changes: &[Change],
    root: Digest,
    accounts: u64,
) -> WalkResult<(Digest, u64, Vec<Write>)> {
    if changes.is_empty() {
        return Ok((root, accounts, Vec::new()));
    }
    let mut walk = Walk::new(snapshot);
    let root = walk.update(0, [0; KEY_BYTES], changes)?.at(0);
    let accounts = (accounts + walk.added)
        .checked_sub(walk.removed)
        .ok_or(WalkError::Stale)?;
    Ok((root, accounts, walk.delta))
}

fn build_plan(
    snapshot: &SnapshotWithThreadMode<'_, DB>,
    generation: u64,
    changes: Vec<Change>,
) -> Result<Plan> {
    let leaves = merge(snapshot_leaves(snapshot)?, &changes);
    let mut walk = Walk::new(snapshot);
    let root = walk.build(&leaves).at(0);
    Ok(Plan {
        generation,
        changes,
        root,
        accounts: to_u64(leaves.len()),
        walked: false,
        delta: walk.delta,
    })
}

/// Stages `plan`'s cache writes and marker into `batch`.
pub(crate) fn stage(batch: &mut WriteBatch, plan: &Plan) {
    if !plan.walked {
        batch.delete_range(node_key(0, &[0; KEY_BYTES]).as_slice(), NODES_END);
    }
    for (key, children) in &plan.delta {
        match children {
            Some(children) => batch.put(key, children),
            None => batch.delete(key),
        }
    }
    batch.put(MARKER, marker_value(&plan.root, plan.accounts));
}

/// Whether the stored cache is exactly what a from-scratch build over every
/// committed account would store: the same root and count, the same children
/// at every branch, and no other branch. O(N) reads and hashes, and O(N)
/// memory for the accounts (about 72 B each), all read from one snapshot.
pub(crate) fn holds(db: &DB) -> Result<bool> {
    let snapshot = db.snapshot();
    let Some((claimed, accounts)) = read_marker(&snapshot)? else {
        return Ok(false);
    };
    let leaves = snapshot_leaves(&snapshot)?;
    let mut walk = Walk::new(&snapshot);
    walk.mismatches = Some(0);
    let root = walk.build(&leaves).at(0);
    if root != claimed || accounts != to_u64(leaves.len()) || walk.mismatches != Some(0) {
        return Ok(false);
    }
    let start = node_key(0, &[0; KEY_BYTES]);
    let mut stored = 0_u64;
    for item in snapshot.iterator(IteratorMode::From(&start, rocksdb::Direction::Forward)) {
        let (key, _) = item.map_err(|e| storage_err(&e))?;
        if *key >= *NODES_END {
            break;
        }
        stored += 1;
    }
    Ok(stored == walk.branches)
}

/// Stages the cache's withdrawal: the next commit rebuilds it.
pub(crate) fn stage_invalidate(batch: &mut WriteBatch) {
    batch.delete(MARKER);
}

/// `leaves` with `changes` applied; both sorted and distinct, and so is the
/// result. One linear pass, so a rebuild under a large block stays O(N + T).
fn merge(leaves: Vec<(Key, Value)>, changes: &[Change]) -> Vec<(Key, Value)> {
    let mut out = Vec::with_capacity(leaves.len() + changes.len());
    let mut changes = changes.iter().peekable();
    for (key, value) in leaves {
        while let Some((changed, new)) = changes.next_if(|(changed, _)| *changed < key) {
            out.extend(new.map(|new| (*changed, new)));
        }
        match changes.next_if(|(changed, _)| *changed == key) {
            Some((_, new)) => out.extend(new.map(|new| (key, new))),
            None => out.push((key, value)),
        }
    }
    out.extend(changes.filter_map(|(key, value)| value.map(|value| (*key, value))));
    out
}

/// Whether `batch` may write an account, which the cache follows. An
/// operation the iterator does not report (a `delete_range`, a `merge`) counts
/// as one: the probe cannot see which keys it covers.
pub(crate) fn touches_accounts(batch: &WriteBatch) -> bool {
    struct Probe {
        accounts: bool,
        seen: usize,
    }
    impl rocksdb::WriteBatchIterator for Probe {
        fn put(&mut self, key: &[u8], _: &[u8]) {
            self.seen += 1;
            self.accounts |= key.starts_with(ACCOUNT_PREFIX);
        }
        fn delete(&mut self, key: &[u8]) {
            self.seen += 1;
            self.accounts |= key.starts_with(ACCOUNT_PREFIX);
        }
    }
    let mut probe = Probe {
        accounts: false,
        seen: 0,
    };
    batch.iterate(&mut probe);
    probe.accounts || probe.seen != batch.len()
}

#[cfg(test)]
thread_local! {
    /// Set by tests that hand the walk a damaged cache on purpose, so the
    /// debug cross-check in `StateDB::root_from` lets the repair path run.
    pub(crate) static UNCHECKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether debug builds compare every planned root with the full rebuild.
#[cfg_attr(not(debug_assertions), allow(dead_code))]
pub(crate) fn cross_checked() -> bool {
    #[cfg(test)]
    {
        !UNCHECKED.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        true
    }
}

/// The root after `changes` from the cache, and the seeks it took; `None`
/// without a valid cache. For the scaling tests: no crossover, no writes.
#[cfg(test)]
pub(crate) fn root_after(db: &DB, changes: &[Change]) -> Result<Option<(Digest, u64)>> {
    let snapshot = db.snapshot();
    let Some(_) = read_marker(&snapshot)? else {
        return Ok(None);
    };
    let mut walk = Walk::new(&snapshot);
    match walk.update(0, [0; KEY_BYTES], changes) {
        Ok(sub) => Ok(Some((sub.at(0), walk.reads))),
        Err(WalkError::Stale) => Ok(None),
        Err(WalkError::Node(error)) => Err(error),
    }
}

#[cfg(test)]
#[path = "root_cache_tests.rs"]
mod tests;
