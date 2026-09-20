//! Why a training round, a share, or a parameter was refused.

use thiserror::Error;

/// A refused operation.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum Error {
    /// A parameter outside what the protocol can honour.
    #[error("invalid parameter: {0}")]
    InvalidParameter(&'static str),
    /// An update containing NaN or infinity.
    #[error("update contains a non-finite value")]
    NonFinite,
    /// Quantized values, masks and noise could wrap the 32-bit sum.
    #[error("{nodes} nodes at this clip, scale and noise could overflow the aggregate")]
    Overflow {
        /// Participants the bound was checked for.
        nodes: usize,
    },
    /// A vector of the wrong length.
    #[error("expected dimension {expected}, found {found}")]
    DimensionMismatch {
        /// The round's dimension.
        expected: usize,
        /// The length offered.
        found: usize,
    },
    /// Fewer shares than the threshold.
    #[error("{have} shares, {need} needed")]
    TooFewShares {
        /// Shares offered.
        have: usize,
        /// The threshold.
        need: usize,
    },
    /// A reconstructed seed that does not match its participant's commitment.
    #[error("reconstructed seed for participant {0} does not match its commitment")]
    CommitmentMismatch(u16),
    /// A participant index the round does not have.
    #[error("unknown participant {0}")]
    UnknownParticipant(u16),
    /// A participant submitted twice.
    #[error("participant {0} submitted twice")]
    DuplicateSubmission(u16),
    /// Too few participants survived to unmask the round.
    #[error("{survivors} survivors, threshold {threshold}")]
    BelowThreshold {
        /// Participants whose masked inputs arrived.
        survivors: usize,
        /// The threshold.
        threshold: usize,
    },
    /// A sealed message failed authentication.
    #[error("sealed message failed authentication")]
    AuthenticationFailed,
    /// An attestation report that does not bind this round.
    #[error("attestation: {0}")]
    Attestation(&'static str),
}

/// Convenience alias for this crate's fallible operations.
pub type Result<T> = core::result::Result<T, Error>;
