//! What this crate refuses.

use thiserror::Error;

use crate::params::ETA;

/// The result of anything here.
pub type Result<T> = std::result::Result<T, Error>;

/// A refusal.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum Error {
    /// A vector or an encoding of the wrong size.
    #[error("{what} has length {found}, expected {expected}")]
    Length {
        /// What was being read or built.
        what: &'static str,
        /// The only length accepted.
        expected: usize,
        /// The length that arrived.
        found: usize,
    },

    /// A coefficient with a second encoding: `≥ q` in a commitment, a nibble
    /// past `2η` in an opening.
    #[error("{what} coefficient {index} is not canonically encoded")]
    NonCanonical {
        /// Which structure.
        what: &'static str,
        /// Which coefficient.
        index: usize,
    },

    /// An opening coefficient outside `[-η, η]`. The whole security of a claim
    /// is this check: without it, any `s` and `e = t - A·s` open any lock.
    #[error("opening coefficient {index} is outside the noise bound ±{ETA}")]
    OutOfBound {
        /// Which coefficient, counting `s` then `e`.
        index: usize,
    },

    /// A commitment every coefficient of which is already short, so `s = 0`
    /// and `e = t` open it and anyone can claim.
    #[error("commitment is trivially openable: s = 0, e = t is a valid opening")]
    TriviallyOpenable,

    /// A well-formed opening that does not satisfy `A·s + e = t`.
    #[error("opening does not satisfy A·s + e = t")]
    Mismatch,

    /// Bytes that are not a well-formed record.
    #[error("malformed {what}: {reason}")]
    Malformed {
        /// Which record kind.
        what: &'static str,
        /// Why it is refused.
        reason: &'static str,
    },
}
