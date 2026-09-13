//! What this crate refuses.

use thiserror::Error;

/// The result of anything here.
pub type Result<T> = std::result::Result<T, Error>;

/// A refusal.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// Bytes that are not a well-formed record.
    #[error("malformed {what}: {reason}")]
    Malformed {
        /// Which record kind was being read.
        what: &'static str,
        /// Why it is refused.
        reason: String,
    },

    /// Something exceeds a bound this crate puts on it.
    #[error("{what} exceeds its bound: {found} > {limit}")]
    Oversized {
        /// What was being read or built.
        what: &'static str,
        /// How much arrived.
        found: usize,
        /// The most this crate will handle.
        limit: usize,
    },

    /// A field carries a value the shape does not allow.
    #[error("invalid {field}: {reason}")]
    Invalid {
        /// The field's name.
        field: &'static str,
        /// Why the value is refused.
        reason: String,
    },

    /// Two things that cannot both be true: a holder listed twice, a supply
    /// that does not match the pages, a distribution against a token that
    /// issued nothing.
    #[error("inconsistent {what}: {reason}")]
    Inconsistent {
        /// What was being checked.
        what: &'static str,
        /// Why it does not hold.
        reason: String,
    },
}
