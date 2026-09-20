//! Why a lattice solution was rejected.
//!
//! These are *rule* failures, not chain errors. The crate returns its own type
//! for the same reason [`maya_dex`] does: keeping `custom-l1-node` out of the
//! dependency graph is the point of the crate boundary, and the mapping from
//! "this vector is not in the lattice" to whichever `NodeError` variant a
//! context calls for belongs to the caller.
//!
//! Every variant names one distinguishable way a solution can be wrong. They
//! are kept separate rather than collapsed into a single `Invalid` because a
//! validator that cannot say *which* rule a block broke cannot be debugged, and
//! because the rejection tests assert on the specific variant — a test that
//! only checks "some error" passes when the wrong rule fired.
//!
//! [`maya_dex`]: https://docs.rs/maya-dex

use core::fmt;

/// Why a candidate solution is not a valid proof of work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatticeError {
    /// The dimension is outside [`MIN_DIMENSION`]`..=`[`MAX_DIMENSION`].
    ///
    /// [`MIN_DIMENSION`]: crate::params::MIN_DIMENSION
    /// [`MAX_DIMENSION`]: crate::params::MAX_DIMENSION
    DimensionOutOfRange,
    /// A modulus below 2, which describes no lattice.
    ModulusTooSmall,
    /// The basis carries a number of coefficients other than `dimension - 1`.
    ///
    /// A separate variant from [`Self::WrongVectorLength`] because the two
    /// arrive from different places — the basis from the chain, the vector from
    /// a miner — and conflating them would hide which side was malformed.
    WrongCoefficientCount,
    /// A basis coefficient at or above the modulus.
    ///
    /// Rejected rather than reduced. Two coefficients differing by a multiple
    /// of `q` describe the same lattice, so accepting unreduced values would
    /// give one lattice many encodings, and a miner a free grinding dimension.
    CoefficientNotReduced,
    /// The candidate vector's length is not the lattice dimension.
    WrongVectorLength,
    /// A coordinate whose magnitude exceeds [`MAX_COORDINATE`].
    ///
    /// A bound, not an overflow report: it is what makes the norm computation
    /// exact rather than merely unlikely to wrap. See [`MAX_COORDINATE`].
    ///
    /// [`MAX_COORDINATE`]: crate::verify::MAX_COORDINATE
    CoordinateOutOfRange,
    /// The vector is not a point of the lattice.
    NotInLattice,
    /// The zero vector, which is in every lattice and is short in all of them.
    ///
    /// Its own variant rather than falling out of the norm test, because a
    /// threshold of zero would otherwise admit it and because "the miner sent
    /// nothing" is worth naming.
    ZeroVector,
    /// The vector is in the lattice but longer than the target permits.
    ///
    /// The ordinary outcome of a mining attempt that has not yet succeeded, not
    /// a malformed input.
    NormAboveTarget,
}

impl fmt::Display for LatticeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::DimensionOutOfRange => "lattice dimension outside the permitted range",
            Self::ModulusTooSmall => "modulus must be at least 2",
            Self::WrongCoefficientCount => "basis needs exactly dimension - 1 coefficients",
            Self::CoefficientNotReduced => "basis coefficient is not reduced modulo q",
            Self::WrongVectorLength => "vector length does not match the lattice dimension",
            Self::CoordinateOutOfRange => "vector coordinate exceeds the permitted magnitude",
            Self::NotInLattice => "vector is not a point of the lattice",
            Self::ZeroVector => "the zero vector is not a proof of work",
            Self::NormAboveTarget => "vector norm exceeds the target",
        };
        f.write_str(message)
    }
}

impl core::error::Error for LatticeError {}

/// Convenience alias for this crate's fallible operations.
pub type Result<T> = core::result::Result<T, LatticeError>;
