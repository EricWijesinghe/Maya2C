//! The tree walk behind the root cache (ADR-062a): reads stored branches and
//! accounts from one snapshot, and computes the root and the cache writes a
//! set of account changes implies. The layout it reads is described in the
//! parent module.

use maya_stateless_core::params::KEY_BITS;
use maya_stateless_core::sparse::bit;
use maya_stateless_core::{Blake3, Compress, Key, Value};
use rocksdb::{DB, DBRawIteratorWithThreadMode, SnapshotWithThreadMode};

use super::{
    Change, Children, Digest, KEY_BYTES, NODE_KEY_LEN, NodeKey, PREFIX_LEN, ROOT_CACHE_PREFIX,
    Write,
};
use crate::error::NodeError;
use crate::state::account::ACCOUNT_LEN;
use crate::state::db::ACCOUNT_PREFIX;
use crate::state::merkle::HASH_LEN;

/// A subtree as its parent sees it.
#[derive(Clone, Copy)]
pub(super) enum Sub {
    Empty,
    Leaf(Digest),
    /// The topmost branch below: at `depth`, with `prefix`'s first `depth`
    /// bits, and its own digest.
    Branch {
        depth: usize,
        prefix: Key,
        digest: Digest,
    },
    /// An untouched child of a stored branch: its digest as that branch sees
    /// it, valid only at that child's position.
    Known(Digest),
}

impl Sub {
    const fn is_empty(&self) -> bool {
        matches!(self, Self::Empty)
    }

    /// The digest a branch whose child sits at `level` sees. A branch deeper
    /// down is hashed with the empty digest once per level between, which is
    /// `sparse::subtree`'s rule for a level with one non-empty side.
    pub(super) fn at(self, level: usize) -> Digest {
        match self {
            Self::Empty => Blake3.empty(),
            Self::Leaf(digest) | Self::Known(digest) => digest,
            Self::Branch {
                depth,
                prefix,
                digest,
            } => (level..depth).rev().fold(digest, |below, at| {
                if bit(&prefix, at) {
                    Blake3.internal(&Blake3.empty(), &below)
                } else {
                    Blake3.internal(&below, &Blake3.empty())
                }
            }),
        }
    }
}

/// A stored branch.
#[derive(Clone, Copy)]
struct Top {
    depth: usize,
    prefix: Key,
    children: Children,
}

impl Top {
    fn sub(&self) -> Sub {
        let (left, right) = split_children(&self.children);
        Sub::Branch {
            depth: self.depth,
            prefix: self.prefix,
            digest: Blake3.internal(&left, &right),
        }
    }
}

fn split_children(children: &Children) -> (Digest, Digest) {
    let mut left = [0; HASH_LEN];
    let mut right = [0; HASH_LEN];
    left.copy_from_slice(&children[..HASH_LEN]);
    right.copy_from_slice(&children[HASH_LEN..]);
    (left, right)
}

fn join_children(left: &Digest, right: &Digest) -> Children {
    let mut children = [0; 2 * HASH_LEN];
    children[..HASH_LEN].copy_from_slice(left);
    children[HASH_LEN..].copy_from_slice(right);
    children
}

pub(super) enum WalkError {
    /// The cache disagrees with the accounts it claims to describe.
    Stale,
    Node(NodeError),
}

impl From<NodeError> for WalkError {
    fn from(error: NodeError) -> Self {
        Self::Node(error)
    }
}

pub(super) type WalkResult<T> = core::result::Result<T, WalkError>;

pub(super) fn storage_err(error: &rocksdb::Error) -> NodeError {
    NodeError::Storage(error.to_string())
}

/// `key`'s first `depth` bits, the rest zero: a node's canonical prefix.
pub(super) fn truncate(key: &Key, depth: usize) -> Key {
    let mut out = [0; KEY_BYTES];
    let whole = depth / 8;
    out[..whole].copy_from_slice(&key[..whole]);
    if depth % 8 != 0 {
        out[whole] = key[whole] & !(0xFF >> (depth % 8));
    }
    out
}

pub(super) fn node_key(depth: usize, prefix: &Key) -> NodeKey {
    let mut key = [0; NODE_KEY_LEN];
    key[..PREFIX_LEN].copy_from_slice(ROOT_CACHE_PREFIX);
    key[PREFIX_LEN..PREFIX_LEN + KEY_BYTES].copy_from_slice(&truncate(prefix, depth));
    // A branch has a child on each side, so its depth is at most 255.
    key[NODE_KEY_LEN - 1] = u8::try_from(depth).unwrap_or(u8::MAX);
    key
}

/// The stored branch `key` and `value` describe; `None` for a malformed one.
fn parse_node(key: &[u8], value: &[u8]) -> Option<Top> {
    if key.len() != NODE_KEY_LEN {
        return None;
    }
    let prefix = Key::try_from(&key[PREFIX_LEN..PREFIX_LEN + KEY_BYTES]).ok()?;
    let depth = usize::from(key[NODE_KEY_LEN - 1]);
    let children = Children::try_from(value).ok()?;
    (truncate(&prefix, depth) == prefix).then_some(Top {
        depth,
        prefix,
        children,
    })
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

/// How many leading bits `a` and `b` share.
pub(super) fn common_bits(a: &Key, b: &Key) -> usize {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .map_or(KEY_BITS, |at| {
            at * 8 + (a[at] ^ b[at]).leading_zeros() as usize
        })
}

pub(super) fn to_u64(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// Reads branches and accounts from one snapshot and records the cache writes
/// the walk implies.
pub(super) struct Walk<'s, 'db> {
    pub(super) snapshot: &'s SnapshotWithThreadMode<'db, DB>,
    nodes: DBRawIteratorWithThreadMode<'s, DB>,
    accounts: DBRawIteratorWithThreadMode<'s, DB>,
    pub(super) delta: Vec<Write>,
    /// When on, every branch `build` produces is compared with the stored one
    /// instead of recorded, and this counts the disagreements: the deep check
    /// at chain open.
    pub(super) mismatches: Option<u64>,
    /// Branches `build` produced.
    pub(super) branches: u64,
    /// `RocksDB` seeks: the cost model the scaling tests count.
    pub(super) reads: u64,
    /// Accounts the changes created and removed.
    pub(super) added: u64,
    pub(super) removed: u64,
}

impl<'s, 'db> Walk<'s, 'db> {
    pub(super) fn new(snapshot: &'s SnapshotWithThreadMode<'db, DB>) -> Self {
        Self {
            snapshot,
            nodes: snapshot.raw_iterator(),
            accounts: snapshot.raw_iterator(),
            delta: Vec::new(),
            mismatches: None,
            branches: 0,
            reads: 0,
            added: 0,
            removed: 0,
        }
    }

    /// The topmost stored branch at or below `depth` under `prefix`: the first
    /// node key from `(prefix, depth)` on, if it lies under `prefix`.
    fn top(&mut self, depth: usize, prefix: &Key) -> WalkResult<Option<Top>> {
        self.reads += 1;
        self.nodes.seek(node_key(depth, prefix));
        let (Some(key), Some(value)) = (self.nodes.key(), self.nodes.value()) else {
            self.nodes.status().map_err(|e| storage_err(&e))?;
            return Ok(None);
        };
        if !key.starts_with(ROOT_CACHE_PREFIX) {
            return Ok(None);
        }
        let found = parse_node(key, value).ok_or(WalkError::Stale)?;
        Ok((found.depth >= depth && under(&found.prefix, prefix, depth)).then_some(found))
    }

    /// The one committed account under a prefix no branch is stored under,
    /// if any. A second one means the cache is stale.
    fn lone_leaf(&mut self, depth: usize, prefix: &Key) -> WalkResult<Option<(Key, Value)>> {
        self.reads += 1;
        let mut start = ACCOUNT_PREFIX.to_vec();
        start.extend_from_slice(&prefix[..depth.div_ceil(8)]);
        self.accounts.seek(&start);
        let mut found = None;
        while let (Some(key), Some(value)) = (self.accounts.key(), self.accounts.value()) {
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
            let value = <[u8; ACCOUNT_LEN]>::try_from(value).map_err(|_| WalkError::Stale)?;
            found = Some((address, value));
            self.accounts.next();
        }
        self.accounts.status().map_err(|e| storage_err(&e))?;
        Ok(found)
    }

    /// The committed subtree at `depth` under `prefix`.
    fn lookup(&mut self, depth: usize, prefix: &Key) -> WalkResult<Sub> {
        if let Some(top) = self.top(depth, prefix)? {
            return Ok(top.sub());
        }
        Ok(self
            .lone_leaf(depth, prefix)?
            .map_or(Sub::Empty, |(key, value)| {
                Sub::Leaf(Blake3.leaf(&key, &value))
            }))
    }

    /// The post-state subtree at `depth` under `prefix`, given its sorted
    /// `changes`.
    pub(super) fn update(
        &mut self,
        depth: usize,
        prefix: Key,
        changes: &[Change],
    ) -> WalkResult<Sub> {
        if changes.is_empty() {
            return self.lookup(depth, &prefix);
        }
        if let Some(top) = self.top(depth, &prefix)? {
            return self.descend(changes, top);
        }
        // At most one committed account below, so nothing below is stored:
        // the subtree after the block is that account, unless changed, plus
        // the changes, built directly.
        let lone = self.lone_leaf(depth, &prefix)?;
        let mut leaves: Vec<(Key, Value)> = changes
            .iter()
            .filter_map(|(key, value)| value.map(|value| (*key, value)))
            .collect();
        if let Some((key, value)) = lone
            && changes
                .binary_search_by(|(changed, _)| changed.cmp(&key))
                .is_err()
        {
            let at = leaves.partition_point(|(other, _)| *other < key);
            leaves.insert(at, (key, value));
        }
        self.added += to_u64(leaves.len());
        self.removed += u64::from(lone.is_some());
        Ok(self.build(&leaves))
    }

    /// The post-state of the region whose topmost branch is `top`, given its
    /// sorted, non-empty `changes`.
    fn descend(&mut self, changes: &[Change], top: Top) -> WalkResult<Sub> {
        let (first, last) = (&changes[0].0, &changes[changes.len() - 1].0);
        // Sorted keys: the least shared prefix with `top` is at an end.
        let split = common_bits(first, &top.prefix)
            .min(common_bits(last, &top.prefix))
            .min(top.depth);
        if split < top.depth {
            // Some change leaves the single-child run above `top` at `split`:
            // a new branch there, `top`'s side on one hand and only changes,
            // over no committed account, on the other.
            let point = changes.partition_point(|(key, _)| !bit(key, split));
            let (zeros, ones) = changes.split_at(point);
            let top_right = bit(&top.prefix, split);
            let (same, other) = if top_right {
                (ones, zeros)
            } else {
                (zeros, ones)
            };
            let same = if same.is_empty() {
                top.sub()
            } else {
                self.descend(same, top)?
            };
            let created: Vec<(Key, Value)> = other
                .iter()
                .filter_map(|(key, value)| value.map(|value| (*key, value)))
                .collect();
            self.added += to_u64(created.len());
            let other = self.build(&created);
            let (left, right) = if top_right {
                (other, same)
            } else {
                (same, other)
            };
            return self.join(split, truncate(&top.prefix, split), left, right, None);
        }
        let point = changes.partition_point(|(key, _)| !bit(key, top.depth));
        let (zeros, ones) = changes.split_at(point);
        let (stored_left, stored_right) = split_children(&top.children);
        let left = if zeros.is_empty() {
            Sub::Known(stored_left)
        } else {
            self.update(top.depth + 1, top.prefix, zeros)?
        };
        let right = if ones.is_empty() {
            Sub::Known(stored_right)
        } else {
            self.update(top.depth + 1, with_bit(top.prefix, top.depth), ones)?
        };
        self.join(top.depth, top.prefix, left, right, Some(top.children))
    }

    /// The node at `depth` under `prefix` over `left` and `right`, writing the
    /// branch it now is (or deleting the one it was, `stored`).
    fn join(
        &mut self,
        depth: usize,
        prefix: Key,
        left: Sub,
        right: Sub,
        stored: Option<Children>,
    ) -> WalkResult<Sub> {
        if !left.is_empty() && !right.is_empty() {
            let (left, right) = (left.at(depth + 1), right.at(depth + 1));
            let children = join_children(&left, &right);
            if stored != Some(children) {
                self.put(depth, &prefix, children);
            }
            return Ok(Sub::Branch {
                depth,
                prefix,
                digest: Blake3.internal(&left, &right),
            });
        }
        if stored.is_some() {
            self.delete(depth, &prefix);
        }
        let (kept, side) = if left.is_empty() {
            (right, with_bit(prefix, depth))
        } else {
            (left, prefix)
        };
        match kept {
            // The surviving side was untouched: its structure, not just its
            // digest, is what the node above needs now.
            Sub::Known(digest) => {
                let sub = self.lookup(depth + 1, &side)?;
                if sub.at(depth + 1) == digest {
                    Ok(sub)
                } else {
                    Err(WalkError::Stale)
                }
            }
            other => Ok(other),
        }
    }

    /// `sparse::subtree` over sorted, distinct `leaves`, recording every
    /// branch.
    pub(super) fn build(&mut self, leaves: &[(Key, Value)]) -> Sub {
        match leaves {
            [] => Sub::Empty,
            [(key, value)] => Sub::Leaf(Blake3.leaf(key, value)),
            [(first, _), .., (last, _)] => {
                let depth = common_bits(first, last);
                let prefix = truncate(first, depth);
                let split = leaves.partition_point(|(key, _)| !bit(key, depth));
                let left = self.build(&leaves[..split]).at(depth + 1);
                let right = self.build(&leaves[split..]).at(depth + 1);
                self.branches += 1;
                self.put(depth, &prefix, join_children(&left, &right));
                Sub::Branch {
                    depth,
                    prefix,
                    digest: Blake3.internal(&left, &right),
                }
            }
        }
    }

    fn put(&mut self, depth: usize, prefix: &Key, children: Children) {
        let key = node_key(depth, prefix);
        match &mut self.mismatches {
            // A read error counts as a disagreement: the answer is a rebuild,
            // whose write then reports the storage fault itself.
            Some(mismatches) => {
                if !matches!(self.snapshot.get(key), Ok(Some(bytes)) if bytes == children) {
                    *mismatches += 1;
                }
            }
            None => self.delta.push((key, Some(children))),
        }
    }

    fn delete(&mut self, depth: usize, prefix: &Key) {
        self.delta.push((node_key(depth, prefix), None));
    }
}
