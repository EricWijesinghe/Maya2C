//! The per-block transaction root a leaf commits to, and its inclusion proof.
//!
//! An odd node at the end of a level is promoted unchanged rather than paired
//! with itself: duplicating it would give `[a, b, c]` and `[a, b, c, c]` the
//! same root (the Bitcoin CVE-2012-2459 ambiguity).

use crate::Hash;

const TAG_TX: u8 = 0x10;
const TAG_TX_NODE: u8 = 0x11;
/// Root of a block with no transactions.
const EMPTY: Hash = [0u8; 32];

fn tx_leaf(tx: &[u8]) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(&[TAG_TX]);
    h.update(tx);
    *h.finalize().as_bytes()
}

fn tx_node(left: &Hash, right: &Hash) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(&[TAG_TX_NODE]);
    h.update(left);
    h.update(right);
    *h.finalize().as_bytes()
}

fn levels<T: AsRef<[u8]>>(txs: &[T]) -> Vec<Vec<Hash>> {
    let mut levels = vec![txs.iter().map(|t| tx_leaf(t.as_ref())).collect::<Vec<_>>()];
    while levels.last().is_some_and(|l| l.len() > 1) {
        let below = levels.last().map(Vec::as_slice).unwrap_or_default();
        let next = below
            .chunks(2)
            .map(|pair| match pair {
                [l, r] => tx_node(l, r),
                [only] => *only,
                _ => unreachable!("chunks(2) yields one or two"),
            })
            .collect();
        levels.push(next);
    }
    levels
}

/// Root over a block's transactions, in block order.
pub fn tx_root<T: AsRef<[u8]>>(txs: &[T]) -> Hash {
    levels(txs)
        .last()
        .and_then(|l| l.first().copied())
        .unwrap_or(EMPTY)
}

/// A path from one transaction to its block's [`tx_root`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxProof {
    /// Position in the block.
    pub index: u32,
    /// Transactions in the block; fixes where the promoted nodes are.
    pub count: u32,
    /// Siblings bottom-up, skipping levels where the node was promoted.
    pub siblings: Vec<Hash>,
}

/// Proof for transaction `index`, or `None` if the block has fewer.
pub fn tx_proof<T: AsRef<[u8]>>(txs: &[T], index: usize) -> Option<TxProof> {
    if index >= txs.len() {
        return None;
    }
    let levels = levels(txs);
    let mut siblings = Vec::new();
    let mut i = index;
    for level in &levels[..levels.len() - 1] {
        let sibling = i ^ 1;
        if sibling < level.len() {
            siblings.push(level[sibling]);
        }
        i /= 2;
    }
    Some(TxProof {
        index: u32::try_from(index).ok()?,
        count: u32::try_from(txs.len()).ok()?,
        siblings,
    })
}

impl TxProof {
    /// Whether `tx` sits at `index` under `root`.
    pub fn verify(&self, root: &Hash, tx: &[u8]) -> bool {
        if self.index >= self.count {
            return false;
        }
        let mut hash = tx_leaf(tx);
        let mut i = self.index as usize;
        let mut width = self.count as usize;
        let mut siblings = self.siblings.iter();
        while width > 1 {
            let sibling = i ^ 1;
            if sibling < width {
                let Some(s) = siblings.next() else {
                    return false;
                };
                hash = if i.is_multiple_of(2) { tx_node(&hash, s) } else { tx_node(s, &hash) };
            }
            i /= 2;
            width = width.div_ceil(2);
        }
        siblings.next().is_none() && hash == *root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_transaction_proves_for_every_block_size() {
        for n in 1..40usize {
            let txs: Vec<Vec<u8>> = (0..n).map(|i| i.to_le_bytes().repeat(i + 1)).collect();
            let root = tx_root(&txs);
            for i in 0..n {
                let p = tx_proof(&txs, i).expect("in range");
                assert!(p.verify(&root, &txs[i]), "n={n} i={i}");
                assert!(!p.verify(&root, b"forged"), "n={n} i={i}");
            }
        }
    }

    #[test]
    fn a_duplicated_last_transaction_changes_the_root() {
        let three = [b"a".as_slice(), b"b", b"c"];
        let four = [b"a".as_slice(), b"b", b"c", b"c"];
        assert_ne!(tx_root(&three), tx_root(&four));
    }
}
