//! **RESEARCH.** True threshold lattice signing, behind the
//! `threshold-lattice` feature: Threshold Raccoon (ADR-014).
//!
//! # What "true" means here
//!
//! The rest of this crate reconstructs the chain key for the length of one
//! signature (see the crate docs); `maya_crypto_pq::multisig` never has one
//! key at all, but ships `m` signatures. A threshold scheme is the third
//! option: `n` parties hold shares, any `T` produce partial signatures, and
//! combining them yields one ordinary signature. With [`dkg`] the full key
//! never exists anywhere.
//!
//! # The scheme, and where each part comes from
//!
//! - **Signing is peer-reviewed.** Threshold Raccoon: del Pino, Katsumata, Maller,
//!   Mouhartem, Prest, Saarinen, *Threshold Raccoon: Practical Threshold
//!   Signatures from Standard Lattice Assumptions*, EUROCRYPT 2024 (ePrint
//!   2024/184). Three rounds, `T`-of-`N` up to 1,024, security from
//!   Hint-MLWE and `SelfTargetMSIS` -- the same family as ML-DSA.
//! - **The code follows the authors' reference**, `masksign/ec24-thrc`
//!   (`thrc-py`, commit `8ef114a`), not the paper alone: the domain headers,
//!   the MACs that replace round-two signatures, the modulus. Its known-answer
//!   vectors (`scripts/traccoon_vectors.py`) pin this port bit for bit.
//! - **Key generation without a dealer is not peer-reviewed.** The paper
//!   assumes a trusted dealer; [`protocol::keygen_dealer`] is that, kept
//!   because the vectors come from it. [`dkg`] is this repository's
//!   construction, secure only against parties who follow it.
//!
//! # What it is not
//!
//! A chain signature. A Threshold Raccoon signature is a *Raccoon* signature, and
//! Raccoon is not a NIST standard -- no suite in `maya_crypto_pq`'s registry
//! verifies one, and none should until one is standardised. So this is for a
//! custody quorum's off-chain authorisation (a vault approving a withdrawal,
//! verified by [`protocol::verify`]), not for signing transactions. The
//! threshold scheme that *would* sign transactions is one whose output is a
//! plain FIPS 204 signature; ADR-014 records what has to be true first.

pub mod dkg;
pub mod gauss;
pub(crate) mod hash;
#[cfg(test)]
mod kat;
pub mod ntt_table;
pub mod params;
pub mod protocol;
pub mod ring;

use sha3::{Digest, Sha3_256};

use crate::error::CustodyError;

/// The scheme ADR-014 chose.
pub const SCHEME: &str =
    "TRaccoon-128 (del Pino et al., EUROCRYPT 2024), reference masksign/ec24-thrc@8ef114a";

/// The scheme's name. Kept as a function so a caller that asks is told which
/// scheme it would be running; this module used to refuse here while no
/// scheme was chosen.
///
/// # Errors
///
/// None today. The `Result` stays so that withdrawing the scheme later is not
/// an API break.
pub fn activate() -> Result<&'static str, CustodyError> {
    Ok(SCHEME)
}

/// SHA3-256 over every coefficient as seven little-endian bytes: how the
/// known-answer fixture digests a polynomial vector.
#[must_use]
pub fn digest(v: &[ring::Poly]) -> [u8; 32] {
    let mut hasher = Sha3_256::new();
    for c in v.iter().flatten() {
        hasher.update(&c.to_le_bytes()[..7]);
    }
    hasher.finalize().into()
}
