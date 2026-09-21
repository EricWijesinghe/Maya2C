//! **RESEARCH — no scheme chosen.** True threshold lattice signing, behind the
//! `threshold-lattice` feature (ADR-011).
//!
//! # What "true" means here
//!
//! The rest of this crate reconstructs the chain key for the length of one
//! signature (see the crate docs); `maya_crypto_pq::multisig` never has one
//! key at all, but ships `m` signatures. A threshold scheme is the third
//! option: `n` parties hold shares from a dealerless keygen, any `t` produce
//! *partial* signatures, and aggregation yields one ordinary signature. The
//! full key never exists anywhere, and the chain sees one signature.
//!
//! # Why this module is an interface
//!
//! The brief allows enabling this only once a peer-reviewed scheme is chosen
//! in an ADR. None is: the candidates ADR-011 lists are recent, none is
//! standardised, and the one that outputs plain FIPS 204 signatures scales to
//! a handful of parties. Implementing one here would be new cryptography with
//! a production flag near it. So the feature compiles the contract a chosen
//! scheme must meet, and [`activate`] refuses until [`SCHEME`] names one.

use maya_crypto_pq::suite::SuiteId;

use crate::error::CustodyError;

/// The ADR-chosen scheme. `None` until ADR-011 is superseded by a decision.
pub const SCHEME: Option<&str> = None;

/// What a chosen threshold lattice scheme must provide.
///
/// The output of [`ThresholdScheme::aggregate`] must verify under
/// [`ThresholdScheme::OUTPUT_SUITE`] with the plain registry verifier, so no
/// consensus code learns that a threshold was involved.
pub trait ThresholdScheme {
    /// The registry suite aggregated signatures verify under.
    const OUTPUT_SUITE: SuiteId;
    /// One party's long-lived key share. Must be `ZeroizeOnDrop`.
    type Share: zeroize::ZeroizeOnDrop;
    /// One party's contribution to one signature.
    type Partial;

    /// Dealerless key generation for party `index` of `parties`, threshold
    /// `threshold`; returns the share and the group's public key.
    ///
    /// # Errors
    ///
    /// Whatever the scheme's protocol can fail with.
    fn keygen(
        index: u8,
        threshold: u8,
        parties: u8,
    ) -> Result<(Self::Share, Vec<u8>), CustodyError>;

    /// A partial signature on `message` from one share.
    ///
    /// # Errors
    ///
    /// Whatever the scheme's signing round can fail with.
    fn partial_sign(share: &Self::Share, message: &[u8]) -> Result<Self::Partial, CustodyError>;

    /// Combines `threshold` partials into one signature.
    ///
    /// # Errors
    ///
    /// Too few partials, or a partial that fails its own check.
    fn aggregate(partials: &[Self::Partial], message: &[u8]) -> Result<Vec<u8>, CustodyError>;
}

/// Refuses until an ADR names a scheme.
///
/// # Errors
///
/// [`CustodyError::NoThresholdScheme`] while [`SCHEME`] is `None`.
pub fn activate() -> Result<&'static str, CustodyError> {
    SCHEME.ok_or(CustodyError::NoThresholdScheme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_activates_without_a_chosen_scheme() {
        // Flipping `SCHEME` is the decision; this test is where it shows up.
        assert_eq!(activate(), Err(CustodyError::NoThresholdScheme));
    }
}
