//! Why a stateless transition was refused.
//!
//! Two kinds of refusal, and conflating them is an attack. A [`Violation`] is a
//! statement about the *block*: a transfer broke a rule, and every node that
//! executes it statefully refuses it too. A [`Defect`] is a statement about the
//! *witness*: it was malformed, stale, or did not open a key the block needs.
//! The witness is not covered by the block's id or proof of work, so any relay
//! can corrupt it — a node that marked a block invalid over a defect would let
//! one relay censor an honest block (the attack invariant 24 closed for bodies).

use thiserror::Error;

use crate::params::KEY_BYTES;

/// A refused stateless transition.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// The block breaks a rule.
    #[error("invalid transition: {0}")]
    Invalid(Violation),
    /// The witness cannot decide the block either way.
    #[error("unverifiable witness: {0}")]
    Unverifiable(Defect),
}

impl Error {
    /// Whether this is a verdict on the block rather than on the witness.
    #[must_use]
    pub const fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid(_))
    }
}

impl From<Violation> for Error {
    fn from(violation: Violation) -> Self {
        Self::Invalid(violation)
    }
}

impl From<Defect> for Error {
    fn from(defect: Defect) -> Self {
        Self::Unverifiable(defect)
    }
}

/// A rule a transfer broke. Mirrors the node's stateful refusals one for one.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum Violation {
    /// The transfer's nonce is not the sender's next.
    #[error("nonce for {} is {actual}, expected {expected}", hex(.key))]
    Nonce {
        /// Sender.
        key: [u8; KEY_BYTES],
        /// The sender's stored nonce.
        expected: u64,
        /// The nonce the transfer carried.
        actual: u64,
    },
    /// The sender holds less than the outputs total.
    #[error("{} holds {available}, needs {required}", hex(.key))]
    InsufficientBalance {
        /// Sender.
        key: [u8; KEY_BYTES],
        /// Sum of the outputs.
        required: u64,
        /// The sender's balance.
        available: u64,
    },
    /// An amount, a balance or a nonce would leave `u64`.
    #[error("arithmetic overflow at {}", hex(.key))]
    Overflow {
        /// The account whose arithmetic overflowed.
        key: [u8; KEY_BYTES],
    },
}

/// Why a witness could not decide a transition.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum Defect {
    /// A key the transition reads lies under a subtree the witness left opaque.
    #[error("witness does not open {}", hex(.key))]
    Unopened {
        /// The key.
        key: [u8; KEY_BYTES],
    },
    /// The encoding is truncated, has trailing bytes, or names an unknown tag.
    #[error("malformed witness: {0}")]
    Malformed(&'static str),
    /// The encoding decodes, but not to the one canonical form of its tree.
    #[error("non-canonical witness: {0}")]
    NonCanonical(&'static str),
    /// The encoding is larger than any honest block needs.
    #[error("witness is {found} bytes, limit {limit}")]
    TooLarge {
        /// The cap.
        limit: usize,
        /// The size offered.
        found: usize,
    },
    /// The witness does not reproduce the root it claims to open.
    #[error("witness does not reproduce the pre-state root")]
    RootMismatch,
    /// Leaves handed to the tree builder are not strictly ascending.
    #[error("leaves are not strictly ascending by key")]
    Unsorted,
    /// The block contains something a transfer-only witness cannot execute.
    #[error("block is not stateless-verifiable: {0}")]
    Ineligible(&'static str),
}

/// Hex of the first four key bytes: enough to tell accounts apart in a log.
fn hex(key: &[u8; KEY_BYTES]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(11);
    for byte in &key[..4] {
        let _ = write!(out, "{byte:02x}");
    }
    out.push_str("..");
    out
}

/// Convenience alias for this crate's fallible operations.
pub type Result<T> = core::result::Result<T, Error>;
