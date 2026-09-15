//! The keyed sparse Merkle tree: building a root, and opening keys against it.
//!
//! ## Shape
//!
//! A leaf sits at the position its key's bits spell, most significant first,
//! but only as deep as it needs to: a subtree holding one leaf *is* that leaf.
//! A subtree holding none is the empty digest. So `N` accounts cost about `2N`
//! node digests, and a path is about `log2 N` deep rather than 256.
//!
//! The shape is a pure function of the key set. That is the property the
//! address-ordered dense tree in `src/state/merkle.rs` lacks: there, inserting
//! one account shifts the position of every account after it, so no witness
//! short of the whole set can say what the new root is. Here, inserting a key
//! changes only the nodes on its own path, and the witness for that path is
//! enough to recompute the root.

use crate::compress::{Compress, Key, Value};
use crate::error::Defect;
use crate::params::KEY_BITS;
use crate::partial::{Node, PartialTree};

/// Bit `depth` of `key`, most significant bit of byte 0 first.
///
/// That order is lexicographic byte order, so a slice sorted by key is already
/// partitioned by every bit in turn — which is what lets the builders below
/// split with a binary search instead of a scan.
#[must_use]
pub const fn bit(key: &Key, depth: usize) -> bool {
    key[depth / 8] >> (7 - depth % 8) & 1 == 1
}

/// Root of the tree holding exactly `leaves`.
///
/// # Errors
///
/// [`Defect::Unsorted`] unless `leaves` is strictly ascending by key. Two
/// values under one key would be two trees, and silently keeping either would
/// be a root nobody asked for.
pub fn root<C: Compress>(compress: &C, leaves: &[(Key, Value)]) -> Result<C::Digest, Defect> {
    check_sorted(leaves)?;
    Ok(subtree(compress, leaves, 0))
}

/// A witness opening every key in `touched` against the tree of `leaves`.
///
/// Each touched key is opened to a leaf if present, or to the empty subtree or
/// the other leaf that proves it absent. Everything else collapses to opaque
/// digests.
///
/// # Errors
///
/// As [`root`].
pub fn open<C: Compress>(
    compress: &C,
    leaves: &[(Key, Value)],
    touched: &[Key],
) -> Result<PartialTree<C::Digest>, Defect> {
    check_sorted(leaves)?;
    let mut keys = touched.to_vec();
    keys.sort_unstable();
    keys.dedup();
    Ok(PartialTree::from_root(open_subtree(
        compress, leaves, &keys, 0,
    )))
}

fn check_sorted(leaves: &[(Key, Value)]) -> Result<(), Defect> {
    if leaves.windows(2).all(|pair| pair[0].0 < pair[1].0) {
        Ok(())
    } else {
        Err(Defect::Unsorted)
    }
}

/// Index of the first key whose bit `depth` is set.
fn split_at<T>(items: &[T], depth: usize, key: impl Fn(&T) -> &Key) -> usize {
    items.partition_point(|item| !bit(key(item), depth))
}

fn subtree<C: Compress>(compress: &C, leaves: &[(Key, Value)], depth: usize) -> C::Digest {
    match leaves {
        [] => compress.empty(),
        [(key, value)] => compress.leaf(key, value),
        _ => {
            // Distinct sorted keys differ at some bit below KEY_BITS, so a
            // slice of two or more never reaches this depth.
            debug_assert!(depth < KEY_BITS);
            let split = split_at(leaves, depth, |leaf| &leaf.0);
            let left = subtree(compress, &leaves[..split], depth + 1);
            let right = subtree(compress, &leaves[split..], depth + 1);
            compress.internal(&left, &right)
        }
    }
}

fn open_subtree<C: Compress>(
    compress: &C,
    leaves: &[(Key, Value)],
    keys: &[Key],
    depth: usize,
) -> Node<C::Digest> {
    match leaves {
        [] => Node::Empty,
        _ if keys.is_empty() => Node::Opaque(subtree(compress, leaves, depth)),
        [(key, value)] => Node::Leaf(*key, *value),
        _ => {
            let split = split_at(leaves, depth, |leaf| &leaf.0);
            let key_split = split_at(keys, depth, |key| key);
            Node::Internal(
                Box::new(open_subtree(
                    compress,
                    &leaves[..split],
                    &keys[..key_split],
                    depth + 1,
                )),
                Box::new(open_subtree(
                    compress,
                    &leaves[split..],
                    &keys[key_split..],
                    depth + 1,
                )),
            )
        }
    }
}
