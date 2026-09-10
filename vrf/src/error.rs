//! Why a VRF operation failed.
//!
//! No `thiserror` here, unlike the node. This crate is meant to be linkable
//! into a standalone beacon operator with as little behind it as possible, and
//! a derive macro is a proc-macro dependency in a crate whose whole argument
//! for existing is that it links light.

use core::fmt;

/// A VRF proof could not be produced or could not be believed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VrfError {
    /// A key or proof named a scheme this build does not implement.
    UnknownScheme {
        /// The tag that was not recognised.
        tag: u8,
    },
    /// A byte slice was the wrong length for what it was being decoded as.
    InvalidLength {
        /// What was being decoded.
        what: &'static str,
        /// How many bytes were required.
        expected: usize,
        /// How many were supplied.
        actual: usize,
    },
    /// A compressed point did not decode to a curve point.
    InvalidPoint {
        /// Which field the point came from.
        what: &'static str,
    },
    /// A scalar was not in canonical form — at or above the group order.
    ///
    /// Rejected rather than reduced. Reducing it would make two distinct proof
    /// encodings verify identically, which is malleability: the same randomness
    /// under two different proof hashes.
    NonCanonicalScalar,
    /// A public key is a small-order point.
    ///
    /// Such a key has a tiny effective keyspace, so its "secret" is guessable
    /// and its outputs are not unpredictable to anyone. Refused at registration
    /// and again at verification, because a key that reached storage before the
    /// check existed must not become usable by being loaded.
    SmallOrderKey,
    /// The proof did not verify against the key and input given.
    VerificationFailed,
    /// Hashing to a curve point failed for every counter value.
    ///
    /// Each attempt succeeds with probability about one half, so 256 failures
    /// in a row has probability around 2^-256. Reaching this means the hash
    /// function or the curve arithmetic is broken, not that the input was
    /// unusual.
    HashToCurveExhausted,
}

impl fmt::Display for VrfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownScheme { tag } => write!(f, "unknown VRF scheme tag {tag}"),
            Self::InvalidLength {
                what,
                expected,
                actual,
            } => write!(
                f,
                "invalid {what} length: expected {expected} bytes, got {actual}"
            ),
            Self::InvalidPoint { what } => write!(f, "{what} is not a curve point"),
            Self::NonCanonicalScalar => f.write_str("scalar is not canonically reduced"),
            Self::SmallOrderKey => f.write_str("public key has small order"),
            Self::VerificationFailed => f.write_str("VRF proof did not verify"),
            Self::HashToCurveExhausted => {
                f.write_str("hash-to-curve found no point in 256 attempts")
            }
        }
    }
}

impl core::error::Error for VrfError {}

/// Convenience alias for this crate's fallible operations.
pub type Result<T> = core::result::Result<T, VrfError>;
