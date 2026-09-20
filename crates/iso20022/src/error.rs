//! What can go wrong reading a message somebody else wrote.
//!
//! Every variant names the field or the bound it failed on. A bank rail's first
//! diagnostic is a rejected message, and "malformed XML" is not a diagnostic —
//! the sender has to know which element, and an operator reading a log has to
//! know whether the cause was a hostile payload or a version mismatch.

use thiserror::Error;

/// The result of anything that reads or writes an ISO 20022 message.
pub type Result<T> = std::result::Result<T, Error>;

/// A message this crate refuses.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// The bytes are not well-formed XML, or the reader hit something it will
    /// not process.
    #[error("xml: {0}")]
    Xml(String),

    /// An element the message must carry is absent.
    #[error("missing element: {0}")]
    Missing(&'static str),

    /// An element appears where the schema does not allow it, or twice where it
    /// may appear once.
    #[error("unexpected element {element}: {reason}")]
    Unexpected {
        /// The element's local name.
        element: String,
        /// Why it is refused here.
        reason: &'static str,
    },

    /// A repeated element, a string, or the document itself exceeds the bound
    /// this crate puts on it.
    ///
    /// Separate from [`Error::Xml`] on purpose: a bound is the variant that
    /// distinguishes a hostile message from a malformed one, and the two want
    /// different responses from an operator.
    #[error("{what} exceeds its bound: {found} > {limit}")]
    Bound {
        /// What was being read.
        what: &'static str,
        /// How much arrived.
        found: usize,
        /// The most this crate will read.
        limit: usize,
    },

    /// An amount is not a decimal this bridge settles, or does not divide into
    /// base units exactly. See [`crate::amount`].
    #[error("amount: {0}")]
    Amount(String),

    /// A field carries a value the schema constrains and this one is outside
    /// it: a currency that is not three letters, an IBAN too long to be one, a
    /// date that is not a date.
    #[error("invalid {field}: {reason}")]
    Invalid {
        /// The field's name, as the schema spells it.
        field: &'static str,
        /// Why the value is refused.
        reason: String,
    },

    /// The document is a message this crate does not implement, or a version of
    /// one it does not implement.
    ///
    /// Named rather than folded into [`Error::Xml`] because it is the error a
    /// counterparty upgrade produces, and that is an operations problem rather
    /// than a security one.
    #[error("unsupported message: {0}")]
    Unsupported(String),
}
