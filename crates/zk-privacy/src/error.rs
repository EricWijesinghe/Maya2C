//! Errors raised by the shielded pool primitives.

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, ZkError>;

/// A failure in note handling, tree maintenance, or proving.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ZkError {
    /// A field element encoding was at or above the modulus.
    #[error("field element is not canonically encoded")]
    NonCanonicalField,

    /// A limb carried more than the 128 bits it is allowed.
    #[error("limb {index} exceeds 128 bits")]
    LimbOutOfRange {
        /// Which of the two limbs was out of range.
        index: usize,
    },

    /// The commitment tree is full.
    #[error("the commitment tree is full at {capacity} notes")]
    TreeFull {
        /// Maximum number of leaves.
        capacity: u64,
    },

    /// A Merkle path was requested for a leaf that does not exist.
    #[error("no note at index {index}")]
    UnknownLeaf {
        /// Requested leaf index.
        index: u64,
    },

    /// A path had the wrong number of levels for the tree depth.
    #[error("merkle path has {actual} levels, expected {expected}")]
    PathLength {
        /// Levels supplied.
        actual: usize,
        /// Levels required.
        expected: usize,
    },

    /// Inputs and outputs did not balance.
    #[error("value does not balance: {inputs} in, {outputs} out")]
    ValueImbalance {
        /// Total input value plus public input.
        inputs: u128,
        /// Total output value plus public output plus fee.
        outputs: u128,
    },

    /// A value exceeded the 64-bit range the circuit enforces.
    #[error("value {value} exceeds the 64-bit range")]
    ValueOutOfRange {
        /// The offending value.
        value: u128,
    },

    /// The witness does not satisfy the circuit, so no proof exists for it.
    ///
    /// Most often a Merkle path built against a different tree state than the
    /// anchor names — a wallet whose view of the pool has gone stale.
    #[error("the witness does not satisfy the circuit; check the anchor and Merkle paths")]
    Unsatisfiable,

    /// Proof generation failed.
    #[error("proof generation failed: {0}")]
    Prove(String),

    /// A proof or key could not be decoded.
    #[error("malformed proof or key: {0}")]
    Malformed(String),

    /// The verifying key did not match the pinned hash.
    #[error("verifying key hash mismatch: expected {expected}, got {actual}")]
    VerifyingKeyMismatch {
        /// Hex of the pinned hash.
        expected: String,
        /// Hex of the hash actually produced.
        actual: String,
    },
}
