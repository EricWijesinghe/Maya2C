//! Errors raised by the CUDA miner.
//!
//! Mirrors the shape of `custom_l1_node::error::NodeError`: one variant per
//! thing that can actually go wrong, each carrying the detail a caller would
//! otherwise have to guess at. The crate does not depend on the node, so it
//! cannot reuse that enum, and converting between the two would only matter if
//! the miner ran in-process — which it does not.

/// Everything the miner can fail at.
#[derive(Debug, thiserror::Error)]
pub enum MinerError {
    /// A BLAKE2b instance was constructed with an output size it rejects.
    ///
    /// Only reachable if a constant in this crate is wrong, since every call
    /// site passes a compile-time size. Kept as an error rather than a panic
    /// because a mining process that dies is worse than one that reports.
    #[error("BLAKE2b rejected an output size of {size} bytes: {reason}")]
    Blake2OutputSize { size: usize, reason: String },

    /// A BLAKE2b digest could not be written into the buffer provided.
    #[error("BLAKE2b could not fill a {size}-byte buffer: {reason}")]
    Blake2Finalize { size: usize, reason: String },

    /// A header was not the canonical [`HEADER_LEN`] bytes.
    ///
    /// [`HEADER_LEN`]: crate::hash::HEADER_LEN
    #[error("header must be {expected} bytes, got {actual}")]
    HeaderLength { expected: usize, actual: usize },

    /// The Argon2 lane buffer was not the size the fill loop requires.
    #[error("lane must hold {expected} blocks, got {actual}")]
    LaneLength { expected: usize, actual: usize },

    /// The CUDA shim reported a failure.
    ///
    /// Carries a description rather than the raw status code: the operator's
    /// response to "the dataset does not fit in VRAM" and to "a kernel failed
    /// to launch" are entirely different, and a number tells them neither.
    #[error("CUDA: {0}")]
    Cuda(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, MinerError>;
