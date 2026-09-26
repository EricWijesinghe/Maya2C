//! The note-commitment tree: incremental, depth 32, stored as a frontier.
//!
//! A node keeps only the *frontier* — for each level, the last left child
//! still waiting for a right sibling — plus the leaf count. That is 32
//! digests however many notes exist, and it is enough to append and to
//! compute the root. Wallets, which do need paths, rebuild them from the
//! leaves they watched ([`merkle_path`]).
//!
//! Empty positions are the zero digest and an empty subtree is the hash of
//! two empty ones, so the root of a partly filled tree is well defined.

use p3_field::{PrimeCharacteristicRing as _, PrimeField32 as _};

use crate::ZkError;
use crate::gadgets::merkle::MerklePath;
use crate::hash::{DIGEST, Digest, F, MODULUS, compress};

/// Tree depth: 2^32 notes.
pub const TREE_DEPTH: usize = 32;
/// Leaves the tree can hold.
pub const TREE_CAPACITY: u64 = 1 << TREE_DEPTH;

/// Bytes in an encoded digest: eight little-endian `u32`.
pub const DIGEST_BYTES: usize = 4 * DIGEST;

/// A digest's canonical 32-byte encoding.
#[must_use]
pub fn digest_to_bytes(d: &Digest) -> [u8; DIGEST_BYTES] {
    let mut out = [0u8; DIGEST_BYTES];
    for (chunk, x) in out.as_chunks_mut::<4>().0.iter_mut().zip(d) {
        *chunk = x.as_canonical_u32().to_le_bytes();
    }
    out
}

/// Decodes a digest, refusing any word that is not a canonical field element
/// — one digest, one encoding.
///
/// # Errors
///
/// [`ZkError::Malformed`] for a word at or above the modulus.
pub fn digest_from_bytes(bytes: &[u8; DIGEST_BYTES]) -> Result<Digest, ZkError> {
    let mut out = [F::ZERO; DIGEST];
    for (x, chunk) in out.iter_mut().zip(bytes.as_chunks::<4>().0) {
        let word = u32::from_le_bytes(*chunk);
        if word >= MODULUS {
            return Err(ZkError::Malformed("digest word is not canonical".into()));
        }
        *x = F::from_u32(word);
    }
    Ok(out)
}

/// The root of an empty subtree at each height.
fn empty_roots() -> [Digest; TREE_DEPTH + 1] {
    let mut out = [[F::ZERO; DIGEST]; TREE_DEPTH + 1];
    for level in 0..TREE_DEPTH {
        out[level + 1] = compress(&out[level], &out[level]);
    }
    out
}

/// The root of a tree that holds no notes.
pub fn empty_root() -> Digest {
    empty_roots()[TREE_DEPTH]
}

/// The incremental tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitmentTree {
    frontier: [Option<Digest>; TREE_DEPTH],
    count: u64,
}

impl Default for CommitmentTree {
    fn default() -> Self {
        Self {
            frontier: [None; TREE_DEPTH],
            count: 0,
        }
    }
}

impl CommitmentTree {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuilds a tree from a stored frontier and count.
    #[must_use]
    pub fn from_parts(frontier: [Option<Digest>; TREE_DEPTH], count: u64) -> Self {
        Self { frontier, count }
    }

    /// The frontier, for storage.
    #[must_use]
    pub fn frontier(&self) -> &[Option<Digest>; TREE_DEPTH] {
        &self.frontier
    }

    /// Leaves appended so far.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Appends a leaf; returns its index.
    ///
    /// # Errors
    ///
    /// [`ZkError::Unsatisfied`] when the tree is full.
    pub fn append(&mut self, leaf: Digest) -> Result<u64, ZkError> {
        if self.count >= TREE_CAPACITY {
            return Err(ZkError::Unsatisfied("commitment tree is full"));
        }
        let index = self.count;
        let mut node = leaf;
        for level in 0..TREE_DEPTH {
            if (index >> level) & 1 == 0 {
                self.frontier[level] = Some(node);
                break;
            }
            let left = self.frontier[level].expect("a right child has a stored left sibling");
            node = compress(&left, &node);
        }
        self.count += 1;
        Ok(index)
    }

    /// The root.
    ///
    /// Folds the frontier bottom-up: at each level set in `count` the stored
    /// left node absorbs the partial subtree to its right (or an empty one);
    /// at each clear level the partial subtree is paired with an empty one.
    pub fn root(&self) -> Digest {
        let empty = empty_roots();
        let mut partial: Option<Digest> = None;
        for level in 0..TREE_DEPTH {
            partial = if (self.count >> level) & 1 == 1 {
                let left = self.frontier[level].expect("a set count bit has a frontier node");
                Some(compress(&left, &partial.unwrap_or(empty[level])))
            } else {
                partial.map(|node| compress(&node, &empty[level]))
            };
        }
        partial.unwrap_or(empty[TREE_DEPTH])
    }
}

/// The authentication path for `leaves[index]` in a tree built from `leaves`.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] if `index` is out of range.
pub fn merkle_path(leaves: &[Digest], index: u64) -> Result<MerklePath, ZkError> {
    let i = usize::try_from(index)
        .ok()
        .filter(|&i| i < leaves.len())
        .ok_or(ZkError::Unsatisfied("leaf index out of range"))?;
    let empty = empty_roots();
    let mut level_nodes: Vec<Digest> = leaves.to_vec();
    let mut siblings = Vec::with_capacity(TREE_DEPTH);
    let mut position = i;
    for level in 0..TREE_DEPTH {
        let sibling = level_nodes
            .get(position ^ 1)
            .copied()
            .unwrap_or(empty[level]);
        siblings.push(sibling);
        level_nodes = level_nodes
            .chunks(2)
            .map(|pair| compress(&pair[0], pair.get(1).unwrap_or(&empty[level])))
            .collect();
        position >>= 1;
    }
    Ok(MerklePath { siblings, index })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(n: u32) -> Digest {
        core::array::from_fn(|i| F::from_u32(n * 8 + i as u32 + 1))
    }

    #[test]
    fn the_frontier_root_matches_a_full_recomputation() {
        let mut tree = CommitmentTree::new();
        assert_eq!(tree.root(), empty_root());
        let mut leaves = Vec::new();
        for n in 0..13 {
            leaves.push(leaf(n));
            tree.append(leaf(n)).expect("append");
            for (i, l) in leaves.iter().enumerate() {
                let path = merkle_path(&leaves, i as u64).expect("path");
                assert_eq!(path.root(l), tree.root(), "leaf {i} of {}", leaves.len());
            }
        }
    }

    #[test]
    fn digests_round_trip_and_non_canonical_words_are_refused() {
        let d = leaf(3);
        assert_eq!(digest_from_bytes(&digest_to_bytes(&d)), Ok(d));
        let mut bad = digest_to_bytes(&d);
        bad[..4].copy_from_slice(&MODULUS.to_le_bytes());
        assert!(digest_from_bytes(&bad).is_err());
    }
}
