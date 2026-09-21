//! The shielded pool's statement — ADR-008.
//!
//! One AIR, [`joinsplit::JoinSplitAir`]: two notes in, two notes out, value
//! entering and leaving through public amounts. Shielding (transparent → note),
//! transfer (notes → notes) and unshielding (notes → transparent) are the same
//! statement with different edges, built by [`wallet`]. The state machine —
//! anchor window, nullifier set, pool balance — belongs to the chain
//! (`crates/node/src/state/shielded.rs`); this crate proves and verifies.

pub mod joinsplit;
pub mod note;
pub mod tree;
pub mod wallet;
pub mod witness;

use crate::{Proof, ZkError};

use joinsplit::JoinSplitAir;
pub use witness::{JoinSplitPublic, JoinSplitWitness, SpendWitness};

/// Anchors a spend may prove against: the last 128 roots.
pub const ANCHOR_WINDOW: usize = 128;

/// Largest proof a chain should accept, in bytes. The measured proof is about
/// 0.3 MB; the ceiling leaves room for encoding variation without letting a
/// peer hand every node an arbitrarily large blob to parse.
pub const MAX_PROOF_BYTES: usize = 1 << 20;

/// Whether the joinsplit AIR has passed an independent audit.
///
/// The STARK has no setup to trust, but an under-constrained AIR has the same
/// consequence the Groth16 setup had: hidden supply no transparent audit can
/// see. The negative tests pin the constraints this tree knows about; they do
/// not prove there is no constraint missing. Value-bearing chains refuse to
/// start while this is `false`.
pub const CIRCUIT_IS_AUDITED: bool = false;

/// Whether the shielded pool can be relied on to secure real value.
///
/// A function rather than a bare constant so consensus code has one call site
/// to gate on, and so a test pinning it cannot be constant-folded away.
#[must_use]
pub fn circuit_is_audited() -> bool {
    CIRCUIT_IS_AUDITED
}

/// Proves a joinsplit.
///
/// # Errors
///
/// [`ZkError::Unsatisfied`] if the witness fails a native check; entropy.
pub fn prove(witness: &JoinSplitWitness) -> Result<(Proof, JoinSplitPublic), ZkError> {
    witness::check(witness)?;
    let public = witness::public_of(witness);
    let proof = crate::prove(
        &JoinSplitAir::default(),
        witness::trace(witness),
        &public.to_field_elements()?,
    )?;
    Ok((proof, public))
}

/// Verifies a joinsplit proof. Whether the anchor is recent and the
/// nullifiers are fresh are questions for the chain's state.
///
/// # Errors
///
/// [`ZkError::Malformed`] for an oversized proof or bad bytes, the public
/// amounts' range, or [`ZkError::Rejected`].
pub fn verify(proof: &Proof, public: &JoinSplitPublic) -> Result<(), ZkError> {
    verify_bytes(proof.as_bytes(), public)
}

/// [`verify`] over a borrowed encoding — what the node calls, since the proof
/// already sits inside a decoded transaction.
///
/// # Errors
///
/// As [`verify`].
pub fn verify_bytes(proof: &[u8], public: &JoinSplitPublic) -> Result<(), ZkError> {
    if proof.len() > MAX_PROOF_BYTES {
        return Err(ZkError::Malformed("proof exceeds MAX_PROOF_BYTES".into()));
    }
    crate::proof::verify_bytes(
        &JoinSplitAir::default(),
        proof,
        &public.to_field_elements()?,
    )
}

#[cfg(test)]
mod tests;
