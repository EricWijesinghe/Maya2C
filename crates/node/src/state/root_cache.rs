//! The incremental sparse accounts root (ADR-062a).
//!
//! ## Why
//!
//! `sparse_accounts_root` reads every account and hashes the whole tree, every
//! block. That is O(all accounts) for the producer and every validator, and an
//! attacker grows "all accounts" for the price of dust outputs
//! (`reports/sessions/2026-11-13-per-block-scan-bounds.md`).
//!
//! ## What is cached
//!
//! The digest of every node of `maya-stateless-core`'s sparse tree whose
//! subtree holds two or more accounts, under `smt:` + depth + the node's key
//! bits. A subtree with one account *is* that account's leaf and one with none
//! is the empty digest, so neither is stored: an absent entry means "at most
//! one account below here", and the account (if any) is one `RocksDB` seek
//! into the `acct:` range the node's bits spell. Updating a block's touched
//! accounts rehashes only their paths, at most 256 nodes each, whatever the
//! number of accounts. No bucket depth is fixed, because recipients are chosen
//! freely: a fixed-depth cache is one an attacker fills a single bucket of.
//!
//! ## When it is trusted
//!
//! The bare key `smt:` holds the committed accounts root the entries describe.
//! It is present only while the cache matches committed state:
//!
//! - every commit path (`apply_block`, `apply_block_checked`,
//!   `apply_journaled_with`, `revert_block_with`) stages the cache delta in the
//!   same `WriteBatch` as the accounts it follows, so a crash leaves both or
//!   neither (invariant 26's rule, applied to a derived record);
//! - every other write that can touch `acct:` (`put_account`, `raw_put`,
//!   `write_batch`: genesis seeding, snapshot import, tests) deletes it, and
//!   the next commit rebuilds the cache from scratch;
//! - a commit whose post-state is not sparse deletes it, so a reorg across the
//!   activation height cannot leave a stale cache waiting for the next one;
//! - `verify_root_cache`, run when a chain opens, compares it with a full
//!   rebuild and rebuilds on any difference.
//!
//! It is not state: two nodes with different caches compute the same root, and
//! `smt:` is on `LOCAL_ONLY_PREFIXES` (invariant 25), so no snapshot carries it.
//! The full rebuild (`StateDB::state_root`) stays the reference, and the root
//! value is the full rebuild's, bit for bit.

use maya_stateless_core::params::KEY_BITS;
use maya_stateless_core::sparse::bit;
use maya_stateless_core::{Blake3, Compress, Key, Value};
use rocksdb::{DB, IteratorMode, SnapshotWithThreadMode, WriteBatch};

use crate::error::{NodeError, Result};
use crate::state::account::ACCOUNT_LEN;
use crate::state::db::ACCOUNT_PREFIX;
use crate::state::merkle::HASH_LEN;

/// Prefix of the cache. Local-only (invariant 25).
pub(crate) const ROOT_CACHE_PREFIX: &[u8] = b"smt:";

/// The marker: present iff the cache describes committed state. Its value is
/// the committed sparse accounts root. Node keys are longer, so it is unique.
const MARKER: &[u8] = ROOT_CACHE_PREFIX;

/// First key after every node key (`;` follows `:`), for `delete_range`.
const NODES_END: &[u8] = b"smt;";

type Digest = [u8; HASH_LEN];

/// One account change: `None` deletes the account (a revert).
pub(crate) type Change = (Key, Option<Value>);

/// A subtree as its parent sees it.
#[derive(Clone, Copy)]
enum Sub {
    Empty,
    Leaf(Digest),
    Branch(Digest),
}

impl Sub {
    fn digest(self) -> Digest {
        match self {
            Self::Empty => Blake3.empty(),
            Self::Leaf(digest) | Self::Branch(digest) => digest,
        }
    }

    /// `sparse::subtree`'s rule: one leaf is that leaf, two or more hash.
    fn join(left: Self, right: Self) -> Self {
        match (left, right) {
            (Self::Empty, Self::Empty) => Self::Empty,
            (Self::Leaf(d), Self::Empty) | (Self::Empty, Self::Leaf(d)) => Self::Leaf(d),
            _ => Self::Branch(Blake3.internal(&left.digest(), &right.digest())),
        }
    }
}

/// The cache disagrees with the accounts it claims to describe.
pub(crate) struct Stale;

enum WalkError {
    Stale,
    Node(NodeError),
}

impl From<NodeError> for WalkError {
    fn from(error: NodeError) -> Self {
        Self::Node(error)
    }
}

fn storage_err(error: &rocksdb::Error) -> WalkError {
    WalkError::Node(NodeError::Storage(error.to_string()))
}

fn node_key(depth: usize, prefix: &Key) -> Vec<u8> {
    let bytes = depth.div_ceil(8);
    let mut key = Vec::with_capacity(ROOT_CACHE_PREFIX.len() + 1 + bytes);
    key.extend_from_slice(ROOT_CACHE_PREFIX);
    // A cached node holds two distinct keys, so it sits above depth 256.
    key.push(u8::try_from(depth).unwrap_or(u8::MAX));
    key.extend_from_slice(&prefix[..bytes]);
    key
}

fn with_bit(mut prefix: Key, depth: usize) -> Key {
    prefix[depth / 8] |= 0x80 >> (depth % 8);
    prefix
}

/// Whether `key` agrees with `prefix` on its first `depth` bits.
fn under(key: &Key, prefix: &Key, depth: usize) -> bool {
    let whole = depth / 8;
    let rest = depth % 8;
    key[..whole] == prefix[..whole]
        && (rest == 0 || (key[whole] ^ prefix[whole]) & !(0xFF >> rest) == 0)
}

/// Reads cache entries and accounts from one consistent snapshot and, when
/// `delta` is on, records the cache writes the walk implies.
struct Walk<'a> {
    snapshot: SnapshotWithThreadMode<'a, DB>,
    delta: Option<Vec<(Vec<u8>, Option<Digest>)>>,
    /// When on, every node `build` produces is compared with the stored one,
    /// and this counts the disagreements: the deep check at chain open.
    mismatches: Option<u64>,
    /// Nodes `build` found with two or more accounts.
    branches: u64,
    /// Nodes visited: what the scaling test counts.
    visited: u64,
}

impl<'a> Walk<'a> {
    fn new(db: &'a DB, record: bool) -> Self {
        Self {
            snapshot: db.snapshot(),
            delta: record.then(Vec::new),
            mismatches: None,
            branches: 0,
            visited: 0,
        }
    }

    fn cached(
        &self,
        depth: usize,
        prefix: &Key,
    ) -> core::result::Result<Option<Digest>, WalkError> {
        if depth >= KEY_BITS {
            return Ok(None);
        }
        match self
            .snapshot
            .get(node_key(depth, prefix))
            .map_err(|e| storage_err(&e))?
        {
            None => Ok(None),
            Some(bytes) => Digest::try_from(bytes.as_slice())
                .map(Some)
                .map_err(|_| WalkError::Stale),
        }
    }

    /// The one committed account under a node with no cache entry, if any.
    /// A second one means the cache is stale.
    fn lone_leaf(
        &self,
        depth: usize,
        prefix: &Key,
    ) -> core::result::Result<Option<(Key, Value)>, WalkError> {
        let mut start = ACCOUNT_PREFIX.to_vec();
        start.extend_from_slice(&prefix[..depth.div_ceil(8)]);
        let mode = IteratorMode::From(start.as_slice(), rocksdb::Direction::Forward);
        let mut found = None;
        for item in self.snapshot.iterator(mode) {
            let (key, value) = item.map_err(|e| storage_err(&e))?;
            let Some(raw) = key.strip_prefix(ACCOUNT_PREFIX) else {
                break;
            };
            let address = Key::try_from(raw).map_err(|_| WalkError::Stale)?;
            if !under(&address, prefix, depth) {
                break;
            }
            if found.is_some() {
                return Err(WalkError::Stale);
            }
            let value = <[u8; ACCOUNT_LEN]>::try_from(&*value).map_err(|_| WalkError::Stale)?;
            found = Some((address, value));
        }
        Ok(found)
    }

    fn put(&mut self, depth: usize, prefix: &Key, digest: Digest) {
        if let Some(delta) = &mut self.delta {
            delta.push((node_key(depth, prefix), Some(digest)));
        }
        if let Some(mismatches) = &mut self.mismatches {
            // A read error counts as a disagreement: the answer is a rebuild,
            // whose write then reports the storage fault itself.
            let stored = self.snapshot.get(node_key(depth, prefix));
            if !matches!(stored, Ok(Some(bytes)) if bytes == digest) {
                *mismatches += 1;
            }
        }
    }

    fn delete(&mut self, depth: usize, prefix: &Key) {
        if let Some(delta) = &mut self.delta {
            delta.push((node_key(depth, prefix), None));
        }
    }

    /// The post-state subtree at `depth`/`prefix`, given its sorted `changes`.
    fn update(
        &mut self,
        depth: usize,
        prefix: Key,
        changes: &[Change],
    ) -> core::result::Result<Sub, WalkError> {
        self.visited += 1;
        let cached = self.cached(depth, &prefix)?;
        if changes.is_empty() {
            return Ok(match cached {
                Some(digest) => Sub::Branch(digest),
                None => self
                    .lone_leaf(depth, &prefix)?
                    .map_or(Sub::Empty, |(key, value)| {
                        Sub::Leaf(Blake3.leaf(&key, &value))
                    }),
            });
        }
        let Some(old) = cached else {
            // At most one committed account below: the subtree after the block
            // is that account, unless changed, plus the changes. Small, so built
            // directly, and nothing below needs deleting.
            let mut leaves: Vec<(Key, Value)> = changes
                .iter()
                .filter_map(|(key, value)| value.map(|value| (*key, value)))
                .collect();
            if let Some((key, value)) = self.lone_leaf(depth, &prefix)?
                && changes
                    .binary_search_by(|(changed, _)| changed.cmp(&key))
                    .is_err()
            {
                let at = leaves.partition_point(|(other, _)| *other < key);
                leaves.insert(at, (key, value));
            }
            return Ok(self.build(depth, prefix, &leaves));
        };
        let split = changes.partition_point(|(key, _)| !bit(key, depth));
        let left = self.update(depth + 1, prefix, &changes[..split])?;
        let right = self.update(depth + 1, with_bit(prefix, depth), &changes[split..])?;
        let joined = Sub::join(left, right);
        match joined {
            Sub::Branch(digest) if digest == old => {}
            Sub::Branch(digest) => self.put(depth, &prefix, digest),
            Sub::Empty | Sub::Leaf(_) => self.delete(depth, &prefix),
        }
        Ok(joined)
    }

    /// `sparse::subtree` over sorted, distinct `leaves`, recording every node
    /// that holds two or more.
    fn build(&mut self, depth: usize, prefix: Key, leaves: &[(Key, Value)]) -> Sub {
        self.visited += 1;
        match leaves {
            [] => Sub::Empty,
            [(key, value)] => Sub::Leaf(Blake3.leaf(key, value)),
            _ => {
                let split = leaves.partition_point(|(key, _)| !bit(key, depth));
                let left = self.build(depth + 1, prefix, &leaves[..split]);
                let right = self.build(depth + 1, with_bit(prefix, depth), &leaves[split..]);
                let digest = Blake3.internal(&left.digest(), &right.digest());
                self.branches += 1;
                self.put(depth, &prefix, digest);
                Sub::Branch(digest)
            }
        }
    }
}

fn walk_error(error: WalkError) -> core::result::Result<Stale, NodeError> {
    match error {
        WalkError::Stale => Ok(Stale),
        WalkError::Node(error) => Err(error),
    }
}

/// The committed root the cache claims, or `None` when it claims nothing.
pub(crate) fn marker(db: &DB) -> Result<Option<Digest>> {
    Ok(db
        .get(MARKER)
        .map_err(|e| NodeError::Storage(e.to_string()))?
        .and_then(|bytes| Digest::try_from(bytes.as_slice()).ok()))
}

/// The sparse accounts root after `changes` (sorted, distinct), from the
/// cache. `Ok(None)` when there is no valid cache to answer from; the caller
/// then takes the full rebuild. Writes nothing.
pub(crate) fn root_after(db: &DB, changes: &[Change]) -> Result<Option<(Digest, u64)>> {
    if marker(db)?.is_none() {
        return Ok(None);
    }
    let mut walk = Walk::new(db, false);
    match walk.update(0, [0; 32], changes) {
        Ok(sub) => Ok(Some((sub.digest(), walk.visited))),
        Err(error) => walk_error(error).map(|Stale| None),
    }
}

/// Stages into `batch` the cache for the state `changes` (sorted, distinct)
/// lead to: incrementally from a valid cache, otherwise rebuilt from every
/// account in `all` (called only then). Returns the root it staged.
pub(crate) fn stage(
    db: &DB,
    batch: &mut WriteBatch,
    changes: &[Change],
    all: impl FnOnce() -> Result<Vec<(Key, Value)>>,
) -> Result<Digest> {
    if marker(db)?.is_some() {
        let mut walk = Walk::new(db, true);
        match walk.update(0, [0; 32], changes) {
            Ok(sub) => {
                write_delta(batch, walk.delta.unwrap_or_default());
                let root = sub.digest();
                batch.put(MARKER, root);
                return Ok(root);
            }
            Err(error) => {
                let Stale = walk_error(error)?;
                eprintln!("state: sparse root cache disagrees with accounts; rebuilding it");
            }
        }
    }
    let leaves = merge(all()?, changes);
    Ok(stage_rebuild(db, batch, &leaves))
}

/// Whether the stored cache is exactly what a from-scratch build over
/// `leaves` (every committed account, sorted) would store: the same root, the
/// same digest at every node, and no other node. O(N) reads and hashes, no
/// O(N) memory, so it can run at every chain open.
pub(crate) fn holds(db: &DB, leaves: &[(Key, Value)]) -> Result<bool> {
    let Some(claimed) = marker(db)? else {
        return Ok(false);
    };
    let mut walk = Walk::new(db, false);
    walk.mismatches = Some(0);
    let root = walk.build(0, [0; 32], leaves).digest();
    if root != claimed || walk.mismatches != Some(0) {
        return Ok(false);
    }
    let start = node_key(0, &[0; 32]);
    let mut stored = 0_u64;
    for item in walk.snapshot.iterator(IteratorMode::From(
        start.as_slice(),
        rocksdb::Direction::Forward,
    )) {
        let (key, _) = item.map_err(|e| NodeError::Storage(e.to_string()))?;
        if *key >= *NODES_END {
            break;
        }
        stored += 1;
    }
    Ok(stored == walk.branches)
}

/// Stages a from-scratch cache over `leaves` (sorted, distinct).
pub(crate) fn stage_rebuild(db: &DB, batch: &mut WriteBatch, leaves: &[(Key, Value)]) -> Digest {
    let mut walk = Walk::new(db, true);
    let root = walk.build(0, [0; 32], leaves).digest();
    batch.delete_range(node_key(0, &[0; 32]), NODES_END.to_vec());
    write_delta(batch, walk.delta.unwrap_or_default());
    batch.put(MARKER, root);
    root
}

/// Stages the cache's withdrawal: the next commit rebuilds it.
pub(crate) fn stage_invalidate(batch: &mut WriteBatch) {
    batch.delete(MARKER);
}

fn write_delta(batch: &mut WriteBatch, delta: Vec<(Vec<u8>, Option<Digest>)>) {
    for (key, digest) in delta {
        match digest {
            Some(digest) => batch.put(key, digest),
            None => batch.delete(key),
        }
    }
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

/// Whether any key `batch` writes is an account, which the cache follows.
pub(crate) fn touches_accounts(batch: &WriteBatch) -> bool {
    struct Probe(bool);
    impl rocksdb::WriteBatchIterator for Probe {
        fn put(&mut self, key: &[u8], _: &[u8]) {
            self.0 |= key.starts_with(ACCOUNT_PREFIX);
        }
        fn delete(&mut self, key: &[u8]) {
            self.0 |= key.starts_with(ACCOUNT_PREFIX);
        }
    }
    let mut probe = Probe(false);
    batch.iterate(&mut probe);
    probe.0
}

#[cfg(test)]
#[path = "root_cache_tests.rs"]
mod tests;
