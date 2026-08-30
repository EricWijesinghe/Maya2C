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
