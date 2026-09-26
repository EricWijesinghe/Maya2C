//! The mountain range: an O(log n) appender for pruned nodes, a full store for
//! archives, and the inclusion proof that joins them.
//!
//! The forest is the binary decomposition of the leaf count: 11 leaves are
//! perfect trees of 8, 2 and 1, left to right. Appending a leaf merges equal
//! trees exactly as incrementing a binary counter carries.

use crate::{BlockLeaf, Hash, TAG_BAG, node_hash};

/// The compact form of all history: what a pruned node keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Commitment {
    /// Number of blocks committed to.
    pub leaves: u64,
    /// The bagged peaks.
    pub root: Hash,
}

/// Bags peaks (tallest first) under the leaf count.
///
/// The count is hashed in because the peak *shape* depends on it: without it
/// a range of 3 leaves and a range of 2 whose peaks happened to collide would
/// share a root.
fn bag(leaves: u64, peaks: &[Hash]) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(&[TAG_BAG]);
    h.update(&leaves.to_le_bytes());
    for peak in peaks {
        h.update(peak);
    }
    *h.finalize().as_bytes()
}

/// Heights of the perfect trees for `leaves` leaves, tallest first.
fn peak_heights(leaves: u64) -> impl Iterator<Item = u32> {
    (0..u64::BITS)
        .rev()
        .filter(move |bit| leaves >> bit & 1 == 1)
}

/// A node position as a vector index. Positions come from the archive's own
/// leaf count, which is a `Vec` length, so they always fit.
fn index_of(position: u64) -> usize {
    usize::try_from(position).unwrap_or(usize::MAX)
}

/// Appends blocks keeping only the peaks: at most 64 hashes, whatever the
/// chain length. What a pruned node runs.
#[derive(Clone, Debug, Default)]
pub struct Compactor {
    leaves: u64,
    /// (tree height, root), tallest first.
    peaks: Vec<(u32, Hash)>,
}

impl Compactor {
    /// An empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one more block in. Amortised O(1) hashes.
    pub fn append(&mut self, leaf: &BlockLeaf) {
        let mut carry = (0u32, leaf.hash());
        while let Some(&(height, left)) = self.peaks.last() {
            if height != carry.0 {
                break;
            }
            self.peaks.pop();
            carry = (height + 1, node_hash(&left, &carry.1));
        }
        self.peaks.push(carry);
        self.leaves += 1;
    }

    /// Blocks folded in so far.
    pub fn len(&self) -> u64 {
        self.leaves
    }

    /// Whether nothing has been folded in.
    pub fn is_empty(&self) -> bool {
        self.leaves == 0
    }

    /// Hashes held, which is the pruned node's whole memory cost for history.
    pub fn peak_count(&self) -> usize {
        self.peaks.len()
    }

    /// The compact commitment to everything appended.
    pub fn commitment(&self) -> Commitment {
        let peaks: Vec<Hash> = self.peaks.iter().map(|(_, h)| *h).collect();
        Commitment {
            leaves: self.leaves,
            root: bag(self.leaves, &peaks),
        }
    }
}

/// Every node of the range, level by level. What an archive keeps, so it can
/// prove any block.
#[derive(Clone, Debug, Default)]
pub struct ArchiveMmr {
    /// `levels[0]` are leaf hashes; `levels[j][i]` is the root of leaves
    /// `i·2^j .. (i+1)·2^j`, present once that whole subtree exists.
    levels: Vec<Vec<Hash>>,
}

impl ArchiveMmr {
    /// An empty archive.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one block.
    pub fn append(&mut self, leaf: &BlockLeaf) {
        let mut hash = leaf.hash();
        let mut level = 0;
        loop {
            if self.levels.len() == level {
                self.levels.push(Vec::new());
            }
            self.levels[level].push(hash);
            let len = self.levels[level].len();
            if len % 2 == 1 {
                break;
            }
            hash = node_hash(&self.levels[level][len - 2], &self.levels[level][len - 1]);
            level += 1;
        }
    }

    /// Blocks held.
    pub fn len(&self) -> u64 {
        self.levels.first().map_or(0, |l| l.len() as u64)
    }

    /// Whether the archive holds nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn peaks(&self) -> Vec<Hash> {
        let leaves = self.len();
        let mut covered = 0u64;
        peak_heights(leaves)
            .map(|height| {
                let peak = self.levels[height as usize][index_of(covered >> height)];
                covered += 1 << height;
                peak
            })
            .collect()
    }

    /// The same commitment a [`Compactor`] fed the same blocks produces.
    pub fn commitment(&self) -> Commitment {
        Commitment {
            leaves: self.len(),
            root: bag(self.len(), &self.peaks()),
        }
    }

    /// A proof that block `index` (0-based) is in the range, or `None` if the
    /// archive does not hold it.
    pub fn prove(&self, index: u64) -> Option<InclusionProof> {
        let leaves = self.len();
        if index >= leaves {
            return None;
        }
        let mut start = 0u64;
        let mut own = None;
        for (position, height) in peak_heights(leaves).enumerate() {
            let size = 1u64 << height;
            if index < start + size {
                own = Some((position, height));
                break;
            }
            start += size;
        }
        let (peak_position, height) = own?;
        let siblings = (0..height)
            .map(|level| {
                let node = index_of(index >> level);
                self.levels[level as usize][node ^ 1]
            })
            .collect();
        let mut other_peaks = self.peaks();
        other_peaks.remove(peak_position);
        Some(InclusionProof {
            leaf_index: index,
            leaves,
            siblings,
            other_peaks,
        })
    }
}

/// A path from one leaf to its peak, plus the other peaks to re-bag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InclusionProof {
    /// Index of the leaf, 0-based.
    pub leaf_index: u64,
    /// Leaf count of the commitment this proof is for.
    pub leaves: u64,
    /// Sibling hashes from the leaf up to (not including) its peak.
    pub siblings: Vec<Hash>,
    /// Every peak except the leaf's own, tallest first.
    pub other_peaks: Vec<Hash>,
}

impl InclusionProof {
    /// Whether `leaf` sits at `leaf_index` under `commitment`.
    ///
    /// Every shape fact is re-derived from the leaf count rather than read
    /// from the proof, so a proof with the right hashes in the wrong shape
    /// fails instead of verifying against a different tree.
    pub fn verify(&self, commitment: &Commitment, leaf: &BlockLeaf) -> bool {
        if self.leaves != commitment.leaves || self.leaf_index >= self.leaves {
            return false;
        }
        let heights: Vec<u32> = peak_heights(self.leaves).collect();
        if self.other_peaks.len() + 1 != heights.len() {
            return false;
        }
        let mut start = 0u64;
        let mut own = None;
        for (position, &height) in heights.iter().enumerate() {
            if self.leaf_index < start + (1u64 << height) {
                own = Some((position, height));
                break;
            }
            start += 1u64 << height;
        }
        let Some((position, height)) = own else {
            return false;
        };
        if self.siblings.len() != height as usize {
            return false;
        }
        let mut hash = leaf.hash();
        for (level, sibling) in self.siblings.iter().enumerate() {
            hash = if (self.leaf_index >> level) & 1 == 0 {
                node_hash(&hash, sibling)
            } else {
                node_hash(sibling, &hash)
            };
        }
        let mut peaks = self.other_peaks.clone();
        peaks.insert(position, hash);
        bag(self.leaves, &peaks) == commitment.root
    }

    /// Encoded size in bytes, the figure a light client pays per lookup.
    pub fn encoded_len(&self) -> usize {
        crate::codec::HEADER_LEN + 32 * (self.siblings.len() + self.other_peaks.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(height: u64) -> BlockLeaf {
        BlockLeaf {
            height,
            block_id: *blake3::hash(&height.to_le_bytes()).as_bytes(),
            tx_root: *blake3::hash(&height.to_be_bytes()).as_bytes(),
        }
    }

    #[test]
    fn compactor_and_archive_agree_at_every_length() {
        let mut compactor = Compactor::new();
        let mut archive = ArchiveMmr::new();
        for h in 0..300 {
            compactor.append(&leaf(h));
            archive.append(&leaf(h));
            assert_eq!(
                compactor.commitment(),
                archive.commitment(),
                "length {}",
                h + 1
            );
            assert_eq!(compactor.peak_count(), (h + 1).count_ones() as usize);
        }
    }

    #[test]
    fn every_leaf_proves_and_no_leaf_proves_at_the_wrong_index() {
        let mut archive = ArchiveMmr::new();
        for h in 0..77 {
            archive.append(&leaf(h));
        }
        let c = archive.commitment();
        for i in 0..77 {
            let proof = archive.prove(i).expect("held");
            assert!(proof.verify(&c, &leaf(i)), "leaf {i}");
            assert!(!proof.verify(&c, &leaf(i + 1)), "leaf {i} as {}", i + 1);
        }
        assert!(archive.prove(77).is_none());
    }

    #[test]
    fn a_proof_for_an_older_commitment_fails_against_a_newer_one() {
        let mut archive = ArchiveMmr::new();
        for h in 0..10 {
            archive.append(&leaf(h));
        }
        let proof = archive.prove(3).expect("held");
        archive.append(&leaf(10));
        assert!(!proof.verify(&archive.commitment(), &leaf(3)));
    }
}
