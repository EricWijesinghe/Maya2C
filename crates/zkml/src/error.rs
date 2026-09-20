//! Every way a model, a key, or a proof can be refused.
//!
//! One distinction matters more than the rest: [`ZkmlError`] is returned for
//! input that is *malformed* — a key that does not parse, a buffer over its
//! bound — while a well-formed proof that simply does not verify is
//! `Ok(false)`. The VM traps on the first and returns `0` on the second, so a
//! contract can refuse a bad proof and carry on, rather than every bad proof
//! aborting the block it arrived in.

use thiserror::Error;

/// The result type every fallible operation here returns.
pub type Result<T> = core::result::Result<T, ZkmlError>;

/// A refusal.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ZkmlError {
    /// A model outside the bounds the circuit's range checks were sized for.
    #[error("model exceeds the circuit's bounds: {0}")]
    ModelOutOfBounds(&'static str),

    /// An input vector of the wrong width.
    #[error("expected {expected} inputs, found {found}")]
    WrongInputWidth {
        /// The model's input width.
        expected: usize,
        /// What was supplied.
        found: usize,
    },

    /// A verifying key that does not parse, or was built for a different
    /// circuit size.
    #[error("malformed verifying key: {0}")]
    MalformedKey(&'static str),

    /// A buffer past its compiled-in bound.
    #[error("{what} of {len} bytes exceeds the {max}-byte bound")]
    Oversized {
        /// Which buffer.
        what: &'static str,
        /// Its length.
        len: usize,
        /// The bound.
        max: usize,
    },

    /// The SRS was derived from a public seed and may not secure value.
    #[error("the zkML setup is not trusted; refusing a value-bearing chain")]
    UntrustedSetup,

    /// Key generation or proving failed.
    #[error("proving failed: {0}")]
    Proving(String),

    /// An ONNX file that is not the model shape this crate proves.
    #[error("unsupported ONNX model: {0}")]
    UnsupportedOnnx(String),
}
