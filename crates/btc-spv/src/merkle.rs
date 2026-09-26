//! Transaction inclusion: a Bitcoin merkle branch from a transaction to a
//! header's merkle root.
//!
//! Two known traps, both handled:
//!
//! - **64-byte transactions.** A 64-byte transaction's txid is
//!   indistinguishable from an interior merkle node, so an attacker can
//!   present an inner node as a "transaction". The verifier takes the raw
//!   transaction bytes, derives the txid itself, and refuses 64-byte input.
//! - **Duplicated last node** (CVE-2012-2459). A branch that pairs a node
//!   with itself is accepted only where Bitcoin's odd-level rule puts it —
//!   at the right edge — which the index and transaction count determine.

use crate::header::sha256d;

/// A branch: siblings from the leaf up, plus where the leaf sits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MerkleProof {
    /// Position of the transaction in the block.
    pub index: u32,
    /// Transactions in the block.
    pub tx_count: u32,
    /// Sibling hashes, bottom-up (internal order).
    pub siblings: Vec<[u8; 32]>,
}

fn pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(left);
    buf[32..].copy_from_slice(right);
    sha256d(&buf)
}

/// Merkle root over txids (Bitcoin's rule: an odd level duplicates its last).
pub fn merkle_root(txids: &[[u8; 32]]) -> [u8; 32] {
    let mut level = txids.to_vec();
    if level.is_empty() {
        return [0; 32];
    }
    while level.len() > 1 {
        level = level
            .chunks(2)
            .map(|c| pair(&c[0], c.get(1).unwrap_or(&c[0])))
            .collect();
    }
    level[0]
}

/// Builds the branch for `index` (for tests and relayers).
pub fn prove(txids: &[[u8; 32]], index: usize) -> Option<MerkleProof> {
    if index >= txids.len() {
        return None;
    }
    let mut level = txids.to_vec();
    let mut i = index;
    let mut siblings = Vec::new();
    while level.len() > 1 {
        let sibling = if i ^ 1 < level.len() {
            level[i ^ 1]
        } else {
            level[i]
        };
        siblings.push(sibling);
        level = level
            .chunks(2)
            .map(|c| pair(&c[0], c.get(1).unwrap_or(&c[0])))
            .collect();
        i /= 2;
    }
    Some(MerkleProof {
        index: u32::try_from(index).ok()?,
        tx_count: u32::try_from(txids.len()).ok()?,
        siblings,
    })
}

impl MerkleProof {
    /// Whether the raw transaction `tx` is committed to by `root`.
    pub fn verify_tx(&self, root: &[u8; 32], tx: &[u8]) -> bool {
        if tx.len() == 64 {
            return false;
        }
        self.verify_txid(root, &sha256d(tx))
    }

    /// Whether `txid` is at `index` under `root`. Prefer [`Self::verify_tx`].
    pub fn verify_txid(&self, root: &[u8; 32], txid: &[u8; 32]) -> bool {
        if self.index >= self.tx_count {
            return false;
        }
        let mut hash = *txid;
        let mut i = self.index as usize;
        let mut width = self.tx_count as usize;
        let mut siblings = self.siblings.iter();
        while width > 1 {
            let Some(s) = siblings.next() else {
                return false;
            };
            let is_last_odd = i ^ 1 >= width;
            if is_last_odd && *s != hash {
                return false; // only the right edge may pair with itself
            }
            if !is_last_odd && *s == hash {
                return false; // a duplicate anywhere else is the CVE shape
            }
            hash = if i.is_multiple_of(2) {
                pair(&hash, s)
            } else {
                pair(s, &hash)
            };
            i /= 2;
            width = width.div_ceil(2);
        }
        siblings.next().is_none() && hash == *root
    }
}
