//! Binary Merkle tree over the account set.
//!
//! ## Construction
//!
//! Leaves are the account records ordered by address, so the root is a pure
//! function of state content and not of insertion order.
//!
//! Leaf and internal hashes carry distinct domain tags. Without them an
//! attacker could present an internal node as if it were a leaf — the classic
//! Merkle second-preimage attack.
//!
//! An odd node at any level is **promoted** to the next level rather than
//! duplicated. Duplicating it is the flaw behind Bitcoin's CVE-2012-2459, where
//! two distinct trees hash to the same root.

use crate::state::account::{Account, Address};

/// Length of a Merkle node hash.
pub const HASH_LEN: usize = 32;

const LEAF_TAG: u8 = 0x00;
const INTERNAL_TAG: u8 = 0x01;

/// Hashes a single account into a Merkle leaf.
#[must_use]
pub fn account_leaf(address: &Address, account: &Account) -> [u8; HASH_LEN] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[LEAF_TAG]);
    hasher.update(address);
    hasher.update(&account.encode());
    *hasher.finalize().as_bytes()
}

/// Combines two child hashes into their parent.
#[must_use]
fn internal_node(left: &[u8; HASH_LEN], right: &[u8; HASH_LEN]) -> [u8; HASH_LEN] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[INTERNAL_TAG]);
    hasher.update(left);
    hasher.update(right);
    *hasher.finalize().as_bytes()
}

/// One step of an inclusion path: the sibling that was hashed alongside.
///
/// A level with an odd count promotes its last node rather than duplicating it,
/// so a promoted node contributes **no step** at that level. That is why a path
/// is shorter than `ceil(log2(n))` for some indices, and why a verifier must
/// not assume a fixed depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathStep {
    /// The sibling hash.
    pub sibling: [u8; HASH_LEN],
    /// Whether the sibling sat on the left.
    ///
    /// Carried rather than derived from the index, because a verifier that
    /// recomputed the index would need the leaf count — and a proof that
    /// depended on the tree's size would be a proof the prover could lie about.
    pub sibling_on_left: bool,
}

/// Computes the Merkle root over `leaves`.
///
/// `leaves` must already be in address order. An empty set roots to all zeros.
#[must_use]
pub fn merkle_root(leaves: &[[u8; HASH_LEN]]) -> [u8; HASH_LEN] {
    if leaves.is_empty() {
        return [0u8; HASH_LEN];
    }

    let mut level = leaves.to_vec();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let (pairs, remainder) = level.as_chunks::<2>();

        for [left, right] in pairs {
            next.push(internal_node(left, right));
        }
        // Promote an unpaired trailing node instead of duplicating it.
        if let Some(odd) = remainder.first() {
            next.push(*odd);
        }

        level = next;
    }

    level[0]
}

/// Builds the inclusion path for the leaf at `index`.
///
/// Returns `None` if the index is out of range. An empty tree and a
/// single-leaf tree both yield an empty path — the leaf *is* the root — which
/// [`verify_path`] handles without a special case.
///
/// The construction mirrors [`merkle_root`] exactly, including the promotion of
/// an odd trailing node. Reimplementing the tree shape here rather than sharing
/// it would be two descriptions of one structure, and the second one would
/// eventually be wrong.
#[must_use]
pub fn merkle_path(leaves: &[[u8; HASH_LEN]], index: usize) -> Option<Vec<PathStep>> {
    if index >= leaves.len() {
        return None;
    }

    let mut level = leaves.to_vec();
    let mut position = index;
    let mut path = Vec::new();

    while level.len() > 1 {
        // Nodes that pair up. A trailing odd node is promoted, not duplicated —
        // duplicating it is the flaw behind CVE-2012-2459, where two distinct
        // trees hash to one root.
        let paired = level.len() - level.len() % 2;

        if position < paired {
            let sibling = position ^ 1;
            path.push(PathStep {
                sibling: level[sibling],
                sibling_on_left: sibling < position,
            });
            position /= 2;
        } else {
            // Promoted: it is hashed with nothing at this level, so the path
            // records nothing and the node keeps its value into the next.
            position = paired / 2;
        }

        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let (pairs, remainder) = level.as_chunks::<2>();
        for [left, right] in pairs {
            next.push(internal_node(left, right));
        }
        if let Some(odd) = remainder.first() {
            next.push(*odd);
        }
        level = next;
    }

    Some(path)
}

/// Recomputes the root a leaf and its path imply.
///
/// The verifier never sees the tree, so this is the whole of what a light
/// client can check: whether *some* tree containing this leaf at this position
/// has the root it was told to expect.
#[must_use]
pub fn verify_path(leaf: &[u8; HASH_LEN], path: &[PathStep]) -> [u8; HASH_LEN] {
    let mut node = *leaf;
    for step in path {
        node = if step.sibling_on_left {
            internal_node(&step.sibling, &node)
        } else {
            internal_node(&node, &step.sibling)
        };
    }
    node
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(count: usize) -> Vec<[u8; HASH_LEN]> {
        (0..count)
            .map(|index| {
                let mut leaf = [0u8; HASH_LEN];
                leaf[0] = index as u8;
                leaf[1] = (index >> 8) as u8;
                leaf
            })
            .collect()
    }

    #[test]
    fn every_leaf_of_every_tree_size_proves_against_the_root() {
        // Sizes 1..40 rather than a couple of hand-picked ones: the promotion
        // rule makes odd counts and counts with odd sub-levels behave
        // differently, and the interesting ones are not the ones a person
        // guesses.
        for count in 1..40usize {
            let set = leaves(count);
            let root = merkle_root(&set);

            for index in 0..count {
                let path = merkle_path(&set, index).expect("in range");
                assert_eq!(
                    verify_path(&set[index], &path),
                    root,
                    "leaf {index} of {count} did not prove"
                );
            }
        }
    }

    #[test]
    fn an_out_of_range_index_has_no_path() {
        let set = leaves(5);
        assert!(merkle_path(&set, 5).is_none());
        assert!(merkle_path(&set, usize::MAX).is_none());
        assert!(merkle_path(&[], 0).is_none());
    }

    #[test]
    fn a_single_leaf_is_its_own_root() {
        let set = leaves(1);
        let path = merkle_path(&set, 0).expect("in range");
        assert!(path.is_empty());
        assert_eq!(verify_path(&set[0], &path), merkle_root(&set));
    }

    #[test]
    fn a_tampered_leaf_does_not_reproduce_the_root() {
        let set = leaves(9);
        let root = merkle_root(&set);
        let path = merkle_path(&set, 4).expect("in range");

        let mut forged = set[4];
        forged[31] ^= 1;
        assert_ne!(verify_path(&forged, &path), root);
    }

    #[test]
    fn a_tampered_sibling_does_not_reproduce_the_root() {
        let set = leaves(9);
        let root = merkle_root(&set);
        let mut path = merkle_path(&set, 4).expect("in range");

        path[0].sibling[0] ^= 1;
        assert_ne!(verify_path(&set[4], &path), root);
    }

    #[test]
    fn swapping_a_siblings_side_does_not_reproduce_the_root() {
        // The side is what makes a path positional. Without it, a leaf could be
        // proved at any index whose siblings happened to match.
        let set = leaves(8);
        let root = merkle_root(&set);
        let mut path = merkle_path(&set, 3).expect("in range");

        path[0].sibling_on_left = !path[0].sibling_on_left;
        assert_ne!(verify_path(&set[3], &path), root);
    }

    #[test]
    fn one_leafs_path_does_not_prove_another_leaf() {
        let set = leaves(16);
        let root = merkle_root(&set);
        let path = merkle_path(&set, 2).expect("in range");

        for (index, leaf) in set.iter().enumerate() {
            if index == 2 {
                continue;
            }
            assert_ne!(
                verify_path(leaf, &path),
                root,
                "leaf {index} verified under leaf 2's path"
            );
        }
    }

    #[test]
    fn a_promoted_node_contributes_no_step() {
        // Three leaves: the third is promoted at the bottom level, so its path
        // is one step where a balanced tree would give two.
        let set = leaves(3);
        assert_eq!(merkle_path(&set, 0).expect("in range").len(), 2);
        assert_eq!(merkle_path(&set, 2).expect("in range").len(), 1);
    }
}
