//! The note-commitment tree: append-only, fixed depth, zero-padded.
//!
//! Leaves are note commitments in insertion order; an empty slot is the zero
//! digest and an empty subtree the hash of two empty ones. Every root the
//! tree has ever had is kept, so a spend proved against a slightly old root —
//! the normal case, since a proof takes time and the tree keeps growing —
//! still verifies.

use std::collections::BTreeSet;

use crate::gadgets::merkle::MerklePath;
use crate::hash::{Digest, F, compress};

/// Tree depth: 2^16 = 65,536 notes. A power of two so the Merkle rows fill a
/// trace exactly.
pub const DEPTH: usize = 16;

/// The tree.
#[derive(Clone, Debug)]
pub struct CommitmentTree {
    leaves: Vec<Digest>,
    empty: Vec<Digest>,
    roots: BTreeSet<[u32; 8]>,
}

fn key(root: &Digest) -> [u32; 8] {
    core::array::from_fn(|i| p3_field::PrimeField32::as_canonical_u32(&root[i]))
}

impl Default for CommitmentTree {
    fn default() -> Self {
        let mut empty = vec![[F::default(); 8]];
        for level in 0..DEPTH {
            empty.push(compress(&empty[level], &empty[level]));
        }
        let mut tree = Self {
            leaves: Vec::new(),
            empty,
            roots: BTreeSet::new(),
        };
        let root = tree.root();
        tree.roots.insert(key(&root));
        tree
    }
}

impl CommitmentTree {
    /// Appends a commitment; returns its index, or `None` when full.
    pub fn append(&mut self, commitment: Digest) -> Option<u64> {
        if self.leaves.len() >= 1 << DEPTH {
            return None;
        }
        self.leaves.push(commitment);
        let root = self.root();
        self.roots.insert(key(&root));
        Some(self.leaves.len() as u64 - 1)
    }

    /// The node at `level` and `index`, computed from the leaves.
    fn node(&self, level: usize, index: usize) -> Digest {
        let span = 1usize << level;
        if index * span >= self.leaves.len() {
            return self.empty[level];
        }
        if level == 0 {
            return self.leaves[index];
        }
        compress(
            &self.node(level - 1, 2 * index),
            &self.node(level - 1, 2 * index + 1),
        )
    }

    /// The current root.
    #[must_use]
    pub fn root(&self) -> Digest {
        self.node(DEPTH, 0)
    }

    /// Whether `root` is, or ever was, this tree's root.
    #[must_use]
    pub fn is_known_root(&self, root: &Digest) -> bool {
        self.roots.contains(&key(root))
    }

    /// The authentication path for the leaf at `index`.
    #[must_use]
    pub fn path(&self, index: u64) -> Option<MerklePath> {
        let i = usize::try_from(index)
            .ok()
            .filter(|&i| i < self.leaves.len())?;
        let siblings = (0..DEPTH)
            .map(|level| self.node(level, (i >> level) ^ 1))
            .collect();
        Some(MerklePath { siblings, index })
    }

    /// Number of leaves.
    #[must_use]
    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    /// Whether the tree holds no leaves.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }
}
