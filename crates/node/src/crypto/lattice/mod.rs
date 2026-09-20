//! Turning a block header into the lattice that block must be mined against.
//!
//! # Status
//!
//! **Research branch. Nothing in consensus calls this.** It is the node-side
//! half of [`maya_lattice_pow`], which carries the verification rules and the
//! reasons they are in a crate of their own. The consensus questions — chiefly
//! that lattice reduction is not progress-free — are open, and are recorded in
//! that crate's documentation and in `docs/lattice-pow.md`.
//!
//! # Why the derivation lives here and not there
//!
//! `maya-lattice-pow` has no dependencies, so that Kani can compile it. That
//! rules out BLAKE3, and therefore rules out deriving anything from a header.
//! The same boundary `maya-dex` lives behind: the arithmetic that decides
//! validity is dependency-free and provable, and the hashing that feeds it
//! happens in `custom-l1-node`, which already links BLAKE3.
//!
//! # Why the miner must not choose the lattice
//!
//! This is the property the whole scheme rests on. Finding a short vector is
//! only hard in a lattice you were handed; a miner who chose the basis would
//! choose one they had already reduced, and mine every block for free.
//!
//! So the coefficients come from the header and nowhere else, through a keyed
//! BLAKE3 XOF. The domain string is consensus in exactly the way the layer
//! domains in [`crate::state::proof`] are: one definition, and both the prover
//! and the verifier reach it.
//!
//! # Rejection sampling, not modular reduction
//!
//! Each coefficient is drawn as a `u32` and kept only if it is below the
//! largest multiple of `q` that fits in `u32`. Taking `draw % q` instead would
//! be biased towards the low residues whenever `q` does not divide `2^32`, and
//! a lattice family with a skewed coefficient distribution is a lattice family
//! whose hardness estimates do not apply to it.
//!
//! The loop is bounded: [`MAX_DRAWS_PER_COEFFICIENT`] attempts, after which
//! derivation fails rather than spinning. With the smallest permitted modulus
//! the rejection probability per draw is below `2^-31`, so the bound is
//! unreachable in practice and exists so that a malformed parameter set cannot
//! hang a validating node.

use std::fmt;

use maya_lattice_pow::{Basis, LatticeError, LatticeParams};

/// BLAKE3 derive-key domain for lattice coefficients.
///
/// Consensus. Changing this string changes every lattice on the chain, and so
/// invalidates every proof of work ever produced under the old one.
pub const COEFFICIENT_DOMAIN: &str = "maya lattice pow coefficients v1";

/// Attempts allowed per coefficient before derivation gives up.
///
/// See the module documentation: this bounds a loop that is statistically
/// certain to terminate on its first or second iteration, so that a parameter
/// set nobody validated cannot turn block validation into a spin.
pub const MAX_DRAWS_PER_COEFFICIENT: u32 = 64;

/// Why a lattice could not be derived from a header.
///
/// A local type rather than a `NodeError` variant, because nothing in consensus
/// calls this yet. Adding a variant to the node's error enum would widen a
/// surface every other module can match on, to describe a failure no code path
/// can currently reach. It gets promoted when — if — this is wired to a fork.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivationError {
    /// Rejection sampling ran out of attempts for one coefficient.
    ///
    /// Statistically unreachable: see [`MAX_DRAWS_PER_COEFFICIENT`]. It exists
    /// so that an unvalidated parameter set fails a block rather than spinning
    /// inside one.
    SamplingExhausted,
    /// The derived coefficients were refused by the verifier crate.
    ///
    /// Unreachable by construction — this function emits exactly
    /// `coefficient_count()` values, every one reduced — so reaching it means a
    /// bug here, and the correct response is to fail the derivation rather than
    /// to hand out a basis the two crates disagree about.
    Rejected(LatticeError),
}

impl fmt::Display for DerivationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SamplingExhausted => {
                f.write_str("lattice coefficient rejection sampling did not terminate")
            }
            Self::Rejected(inner) => write!(f, "derived lattice was refused: {inner}"),
        }
    }
}

impl std::error::Error for DerivationError {}

/// Derives the lattice a header must be mined against.
///
/// `seed` is the header's commitment — in a wired implementation, the hash of
/// everything in the header except the solution itself. This function does not
/// know or check that; it is the caller's job to pass something the miner could
/// not have chosen freely.
///
/// # Errors
///
/// [`DerivationError::SamplingExhausted`] if rejection sampling exhausts
/// [`MAX_DRAWS_PER_COEFFICIENT`] for any coefficient, or
/// [`DerivationError::Rejected`] if the verifier crate refuses the result.
pub fn basis_for_seed(params: LatticeParams, seed: &[u8; 32]) -> Result<Basis, DerivationError> {
    let mut hasher = blake3::Hasher::new_derive_key(COEFFICIENT_DOMAIN);
    hasher.update(seed);
    let mut reader = hasher.finalize_xof();

    let modulus = params.modulus;
    // The largest multiple of `q` that fits in `u32`. Draws at or above this
    // are discarded; keeping them is what would bias the distribution.
    let ceiling = (u32::MAX / modulus) * modulus;

    let mut coefficients = Vec::with_capacity(params.coefficient_count());
    for _ in 0..params.coefficient_count() {
        let mut accepted = None;
        for _ in 0..MAX_DRAWS_PER_COEFFICIENT {
            let mut bytes = [0u8; 4];
            reader.fill(&mut bytes);
            let draw = u32::from_le_bytes(bytes);
            if draw < ceiling {
                accepted = Some(draw % modulus);
                break;
            }
        }
        match accepted {
            Some(value) => coefficients.push(value),
            None => return Err(DerivationError::SamplingExhausted),
        }
    }

    Basis::new(params, coefficients).map_err(DerivationError::Rejected)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn params() -> LatticeParams {
        LatticeParams::new(64, 4_294_967_291).expect("valid parameters")
    }

    #[test]
    fn the_same_seed_always_derives_the_same_lattice() {
        let seed = [7u8; 32];
        assert_eq!(
            basis_for_seed(params(), &seed).expect("derives"),
            basis_for_seed(params(), &seed).expect("derives")
        );
    }

    #[test]
    fn a_one_bit_change_in_the_seed_derives_a_different_lattice() {
        let first = basis_for_seed(params(), &[0u8; 32]).expect("derives");
        let mut seed = [0u8; 32];
        seed[31] = 1;
        let second = basis_for_seed(params(), &seed).expect("derives");
        assert_ne!(first, second);
    }

    #[test]
    fn every_derived_coefficient_is_reduced() {
        let basis = basis_for_seed(params(), &[3u8; 32]).expect("derives");
        assert_eq!(
            basis.coefficients().len(),
            basis.params().coefficient_count()
        );
        assert!(basis.coefficients().iter().all(|&x| x < params().modulus));
    }

    #[test]
    fn a_small_modulus_still_derives_a_full_basis() {
        // A modulus that divides no power of two evenly, so rejection sampling
        // actually rejects rather than accepting every draw.
        let params = LatticeParams::new(16, 97).expect("valid parameters");
        let basis = basis_for_seed(params, &[9u8; 32]).expect("derives");
        assert_eq!(basis.coefficients().len(), 15);
        assert!(basis.coefficients().iter().all(|&x| x < 97));
    }

    #[test]
    fn the_derivation_is_domain_separated_from_a_plain_hash() {
        // A plain BLAKE3 of the seed must not reproduce the coefficients, or
        // the domain string is doing nothing and any other subsystem hashing
        // the same seed would collide with this one.
        let seed = [5u8; 32];
        let basis = basis_for_seed(params(), &seed).expect("derives");

        let mut plain = blake3::Hasher::new();
        plain.update(&seed);
        let mut reader = plain.finalize_xof();
        let mut bytes = [0u8; 4];
        reader.fill(&mut bytes);

        assert_ne!(basis.coefficients()[0], u32::from_le_bytes(bytes));
    }
}
