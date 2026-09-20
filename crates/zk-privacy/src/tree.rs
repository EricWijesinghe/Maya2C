//! The note commitment tree.
//!
//! An append-only Poseidon Merkle tree of fixed depth. Notes are only ever
//! added, never removed — spending a note publishes a nullifier instead, so the
//! tree's history stays intact and old Merkle paths keep verifying.
//!
//! ## Two representations
//!
//! [`CommitmentTree`] keeps only the *frontier*: the pending left sibling at
//! each level. That makes appending and re-rooting `O(depth)` instead of
//! `O(notes)`, which matters because a node re-roots on every block, and it
//! makes a reorg cheap — restoring the frontier and the count is enough to
//! rewind the tree exactly.
//!
//! [`merkle_path`] builds an authentication path from the full leaf set. It is
//! `O(notes)` and belongs to the wallet side, which needs a path to prove a
//! spend and has the leaves anyway.
//!
//! ## Why leaves need no domain tag
//!
//! Internal nodes hash two children with no separator, so an internal node's
//! digest could in principle be presented as a leaf. That is harmless here:
//! spending a leaf requires opening it as a note commitment *and* proving
//! knowledge of the spending key behind its address. Passing off an internal
//! node would mean finding a Poseidon preimage of that specific shape.

use ark_bls12_381::Fr;
use ark_ff::Zero;
use std::sync::OnceLock;

use crate::error::{Result, ZkError};
use crate::hash::hash;
use crate::params::TREE_DEPTH;

/// Maximum notes the tree can hold.
pub const TREE_CAPACITY: u64 = 1u64 << TREE_DEPTH;

/// Root of an entirely empty subtree at each level.
///
/// `empty_roots()[0]` is the empty leaf; `empty_roots()[TREE_DEPTH]` is the
/// root of an empty tree.
fn empty_roots() -> &'static [Fr; TREE_DEPTH + 1] {
    static EMPTY: OnceLock<[Fr; TREE_DEPTH + 1]> = OnceLock::new();
    EMPTY.get_or_init(|| {
        let mut roots = [Fr::zero(); TREE_DEPTH + 1];
        for level in 1..=TREE_DEPTH {
            roots[level] = hash(&[roots[level - 1], roots[level - 1]]);
        }
        roots
    })
}

/// The root of an empty commitment tree.
#[must_use]
pub fn empty_root() -> Fr {
    empty_roots()[TREE_DEPTH]
}

/// An append-only commitment tree held as a frontier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitmentTree {
    /// Pending left sibling at each level, if any.
    filled: [Option<Fr>; TREE_DEPTH],
    /// Number of leaves appended so far.
    count: u64,
}

impl Default for CommitmentTree {
    fn default() -> Self {
        Self::new()
    }
}

impl CommitmentTree {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self {
            filled: [None; TREE_DEPTH],
            count: 0,
        }
    }

    /// Rebuilds a tree from a previously captured frontier.
    ///
    /// This is what makes a reorg reversible: the undo record stores these two
    /// values and restoring them rewinds the tree exactly.
    #[must_use]
    pub fn from_parts(filled: [Option<Fr>; TREE_DEPTH], count: u64) -> Self {
        Self { filled, count }
    }

    /// The pending left siblings.
    #[must_use]
    pub fn frontier(&self) -> &[Option<Fr>; TREE_DEPTH] {
        &self.filled
    }

    /// Number of notes in the tree.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Appends a commitment, returning the index it landed at.
    ///
    /// # Errors
    ///
    /// Returns [`ZkError::TreeFull`] once capacity is reached.
    pub fn append(&mut self, commitment: Fr) -> Result<u64> {
        if self.count >= TREE_CAPACITY {
            return Err(ZkError::TreeFull {
                capacity: TREE_CAPACITY,
            });
        }

        let index = self.count;
        let mut node = commitment;

        // Carry upward while this position is a right child: each 1 bit in the
        // current count means a left sibling is already waiting at that level.
        for level in 0..TREE_DEPTH {
            if (self.count >> level) & 1 == 0 {
                self.filled[level] = Some(node);
                break;
            }
            let left = self.filled[level]
                .expect("a set bit in the count guarantees a pending left sibling");
            node = hash(&[left, node]);
        }

        self.count += 1;
        Ok(index)
    }

    /// The current root.
    #[must_use]
    pub fn root(&self) -> Fr {
        let empty = empty_roots();
        // Walk the path of the *next* free position, which is empty, folding in
        // the pending left siblings where the count says one exists.
        let mut node = empty[0];
        for (level, empty_sibling) in empty.iter().enumerate().take(TREE_DEPTH) {
            node = if (self.count >> level) & 1 == 1 {
                let left = self.filled[level]
                    .expect("a set bit in the count guarantees a pending left sibling");
                hash(&[left, node])
            } else {
                hash(&[node, *empty_sibling])
            };
        }
        node
    }
}

/// An authentication path proving a leaf belongs to a root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerklePath {
    /// Sibling at each level, from the leaf upward.
    pub siblings: Vec<Fr>,
    /// Index of the leaf, whose bits give the left/right turns.
    pub index: u64,
}

impl MerklePath {
    /// Folds `leaf` up the path to the root it implies.
    ///
    /// # Errors
    ///
    /// Returns [`ZkError::PathLength`] if the path is not exactly
    /// [`TREE_DEPTH`] levels.
    pub fn compute_root(&self, leaf: Fr) -> Result<Fr> {
        if self.siblings.len() != TREE_DEPTH {
            return Err(ZkError::PathLength {
                actual: self.siblings.len(),
                expected: TREE_DEPTH,
            });
        }

        let mut node = leaf;
        for (level, sibling) in self.siblings.iter().enumerate() {
            node = if (self.index >> level) & 1 == 0 {
                hash(&[node, *sibling])
            } else {
                hash(&[*sibling, node])
            };
        }
        Ok(node)
    }
}

/// Builds the authentication path for `index` from the full leaf set.
///
/// # Errors
///
/// Returns [`ZkError::UnknownLeaf`] if `index` is past the end of `leaves`.
pub fn merkle_path(leaves: &[Fr], index: u64) -> Result<MerklePath> {
    if index >= leaves.len() as u64 {
        return Err(ZkError::UnknownLeaf { index });
    }

    let empty = empty_roots();
    let mut siblings = Vec::with_capacity(TREE_DEPTH);
    let mut level: Vec<Fr> = leaves.to_vec();
    let mut position = index as usize;

    for empty_sibling in empty.iter().take(TREE_DEPTH) {
        let sibling_index = position ^ 1;
        siblings.push(level.get(sibling_index).copied().unwrap_or(*empty_sibling));

        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut cursor = 0;
        while cursor < level.len() {
            let left = level[cursor];
            let right = level.get(cursor + 1).copied().unwrap_or(*empty_sibling);
            next.push(hash(&[left, right]));
            cursor += 2;
        }

        level = next;
        position >>= 1;
    }

    Ok(MerklePath { siblings, index })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn leaf(value: u64) -> Fr {
        Fr::from(value + 1)
    }

    #[test]
    fn an_empty_tree_roots_to_the_empty_root() {
        assert_eq!(CommitmentTree::new().root(), empty_root());
    }

    #[test]
    fn appending_changes_the_root() {
        let mut tree = CommitmentTree::new();
        let before = tree.root();
        tree.append(leaf(0)).expect("append");
        assert_ne!(tree.root(), before);
    }

    #[test]
    fn indices_are_sequential() {
        let mut tree = CommitmentTree::new();
        for expected in 0..8u64 {
            assert_eq!(tree.append(leaf(expected)).expect("append"), expected);
        }
        assert_eq!(tree.count(), 8);
    }

    #[test]
    fn frontier_root_matches_the_path_built_root() {
        // Two independent implementations of the same tree. Sizes span several
        // powers of two so the odd-node padding is exercised on both sides.
        let mut tree = CommitmentTree::new();
        let mut leaves = Vec::new();

        for size in 1..=17u64 {
            let value = leaf(size);
            tree.append(value).expect("append");
            leaves.push(value);

            let path = merkle_path(&leaves, size - 1).expect("path");
            assert_eq!(
                path.compute_root(value).expect("root"),
                tree.root(),
                "frontier and path disagree at {size} leaves"
            );
        }
    }

    #[test]
    fn every_leaf_authenticates_against_the_root() {
        let mut tree = CommitmentTree::new();
        let leaves: Vec<Fr> = (0..9u64).map(leaf).collect();
        for value in &leaves {
            tree.append(*value).expect("append");
        }
        let root = tree.root();

        for (index, value) in leaves.iter().enumerate() {
            let path = merkle_path(&leaves, index as u64).expect("path");
            assert_eq!(
                path.compute_root(*value).expect("root"),
                root,
                "leaf {index} failed to authenticate"
            );
        }
    }

    #[test]
    fn a_wrong_leaf_does_not_authenticate() {
        let leaves: Vec<Fr> = (0..4u64).map(leaf).collect();
        let mut tree = CommitmentTree::new();
        for value in &leaves {
            tree.append(*value).expect("append");
        }

        let path = merkle_path(&leaves, 2).expect("path");
        assert_ne!(path.compute_root(leaf(99)).expect("root"), tree.root());
    }

    #[test]
    fn restoring_a_frontier_rewinds_the_tree() {
        // The reorg property: capture, append more, restore, and the root must
        // be exactly what it was.
        let mut tree = CommitmentTree::new();
        for value in 0..5u64 {
            tree.append(leaf(value)).expect("append");
        }
        let captured = (*tree.frontier(), tree.count());
        let root_before = tree.root();

        for value in 5..11u64 {
            tree.append(leaf(value)).expect("append");
        }
        assert_ne!(tree.root(), root_before);

        let restored = CommitmentTree::from_parts(captured.0, captured.1);
        assert_eq!(restored.root(), root_before);
        assert_eq!(restored, {
            let mut rebuilt = CommitmentTree::new();
            for value in 0..5u64 {
                rebuilt.append(leaf(value)).expect("append");
            }
            rebuilt
        });
    }

    #[test]
    fn a_path_of_the_wrong_length_is_rejected() {
        let path = MerklePath {
            siblings: vec![Fr::zero(); 3],
            index: 0,
        };
        assert!(matches!(
            path.compute_root(leaf(0)),
            Err(ZkError::PathLength {
                actual: 3,
                expected: TREE_DEPTH
            })
        ));
    }

    #[test]
    fn a_path_for_a_missing_leaf_is_rejected() {
        assert!(matches!(
            merkle_path(&[leaf(0)], 5),
            Err(ZkError::UnknownLeaf { index: 5 })
        ));
    }
}
