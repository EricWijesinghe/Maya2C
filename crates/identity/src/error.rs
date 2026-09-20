//! What this crate refuses.

use thiserror::Error;

/// The result of anything here.
pub type Result<T> = std::result::Result<T, Error>;

/// A refusal.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// A string is not a well-formed `did:maya2c:` identifier.
    #[error("did: {0}")]
    Did(String),

    /// Bytes are not a well-formed record.
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

    /// A document names two things that cannot both be true — two verification
    /// methods with one id, an endpoint repeated, a key epoch that went
    /// backwards.
    #[error("inconsistent {what}: {reason}")]
    Inconsistent {
        /// What was being checked.
        what: &'static str,
        /// Why it does not hold.
        reason: String,
    },
}
