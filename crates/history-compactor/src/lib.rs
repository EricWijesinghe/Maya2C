//! History compaction for pruned nodes.
//!
//! Master Prompt 3 §6 asks for all history folded into one proof, with a
//! single old transaction checkable against that proof plus an archive fetch.
//! This crate builds the part of that which can be built honestly today and
//! says plainly which part it is not.
//!
//! # What it is
//!
//! A **Merkle Mountain Range** over one leaf per block, each leaf binding the
//! block's height, its id and its transaction root. The whole history then
//! compacts to a [`Commitment`]: 32 bytes plus a leaf count, independent of how
//! many blocks there were. A pruned node keeps only that, and the O(log n)
//! peaks it needs to keep appending ([`Compactor`]); an archive keeps every
//! node ([`ArchiveMmr`]) and answers with an [`InclusionProof`] of about
//! `32 · log2(n)` bytes. [`verify_transaction`] then checks one transaction
//! fetched from any archive against the commitment, trusting neither the
//! archive nor the transport.
//!
//! # What it is not
//!
//! It is an **accumulator, not a validity proof**. It proves that a block (and
//! a transaction in it) is *the one the chain committed to*; it does not prove
//! that the history it commits to was *valid*. The brief's "recursive STARK /
//! Nova folding" is the second property. Nova-family folding is curve-based
//! and not post-quantum, and a recursive Plonky3 verifier circuit does not
//! exist in this tree (`crates/zk-stark` proves single statements). So the
//! validity half is **RESEARCH, not started**, and a node that bootstraps from
//! a commitment still trusts whoever gave it that commitment exactly as far as
//! it trusts a snapshot's state root — see `docs/pruning.md`.
//!
//! Hash: BLAKE3 with one-byte domain tags, so a leaf can never be presented
//! as an interior node (the second-preimage attack on untagged Merkle trees).

#![warn(missing_docs)]

mod codec;
mod mmr;
mod txroot;

pub use codec::DecodeError;
pub use mmr::{ArchiveMmr, Commitment, Compactor, InclusionProof};
pub use txroot::{TxProof, tx_proof, tx_root};

/// A 32-byte BLAKE3 digest.
pub type Hash = [u8; 32];

/// Domain tag of an MMR leaf.
const TAG_LEAF: u8 = 0x00;
/// Domain tag of an MMR interior node.
const TAG_NODE: u8 = 0x01;
/// Domain tag of the peak bagging that yields the [`Commitment`] root.
const TAG_BAG: u8 = 0x02;

/// The facts one leaf commits to: enough to find the block in an archive and
/// to check a transaction inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockLeaf {
    /// Height of the block. Bound into the leaf so a proof for block 7 cannot
    /// be replayed as block 8 with the same body.
    pub height: u64,
    /// The block's id (header hash).
    pub block_id: Hash,
    /// The block's transaction root, as [`tx_root`] computes it.
    pub tx_root: Hash,
}

impl BlockLeaf {
    /// The leaf's hash in the mountain range.
    pub fn hash(&self) -> Hash {
        let mut h = blake3::Hasher::new();
        h.update(&[TAG_LEAF]);
        h.update(&self.height.to_le_bytes());
        h.update(&self.block_id);
        h.update(&self.tx_root);
        *h.finalize().as_bytes()
    }
}

/// Why an old transaction was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// The block is not in the committed history (wrong leaf, forged path, or
    /// a proof for a different commitment).
    BlockNotCommitted,
    /// The transaction is not in that block's transaction root.
    TxNotInBlock,
}

impl core::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BlockNotCommitted => f.write_str("block is not in the committed history"),
            Self::TxNotInBlock => f.write_str("transaction is not in the block's transaction root"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Checks one historical transaction against a compact commitment.
///
/// `leaf` and `tx` come from an archive nobody trusts; `commitment` is what
/// the pruned node kept. Both proofs are checked; neither alone is enough.
///
/// # Errors
///
/// [`VerifyError::BlockNotCommitted`] if `block_proof` does not place `leaf`
/// under `commitment`; [`VerifyError::TxNotInBlock`] if `tx_proof` does not
/// place `tx` under `leaf.tx_root`.
pub fn verify_transaction(
    commitment: &Commitment,
    leaf: &BlockLeaf,
    block_proof: &InclusionProof,
    tx: &[u8],
    tx_proof: &TxProof,
) -> Result<(), VerifyError> {
    if !block_proof.verify(commitment, leaf) {
        return Err(VerifyError::BlockNotCommitted);
    }
    if !tx_proof.verify(&leaf.tx_root, tx) {
        return Err(VerifyError::TxNotInBlock);
    }
    Ok(())
}

fn node_hash(left: &Hash, right: &Hash) -> Hash {
    let mut h = blake3::Hasher::new();
    h.update(&[TAG_NODE]);
    h.update(left);
    h.update(right);
    *h.finalize().as_bytes()
}
