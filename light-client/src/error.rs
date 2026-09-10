//! Why a light client refused a header or a proof.

use thiserror::Error;

/// A header could not be accepted, or a proof could not be believed.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LightClientError {
    /// A header's parent is not in the chain.
    ///
    /// A light client follows headers in order. One arriving with no known
    /// parent is either out of sequence or from a chain this client has never
    /// seen, and guessing which would mean accepting a header it cannot place.
    #[error("header {header} has unknown parent {parent}")]
    UnknownParent {
        /// Hex-encoded identifier of the header offered.
        header: String,
        /// Hex-encoded parent it names.
        parent: String,
    },

    /// A header's digest does not satisfy its own target.
    #[error("header {0} does not satisfy its difficulty target")]
    InsufficientWork(String),

    /// A header's target is easier than the network's floor.
    ///
    /// The check that stops a fake chain being cheap. Without it, an attacker
    /// mines a thousand headers at trivial difficulty and offers them as a
    /// longer chain; with it, every header on every chain costs real work.
    #[error("header {header} target is easier than the network floor")]
    TargetBelowFloor {
        /// Hex-encoded identifier of the header offered.
        header: String,
    },

    /// A header was offered for a height the client has already filled with a
    /// different header, and the new one does not carry more work.
    ///
    /// Not an error about the header — it may be perfectly valid — but about
    /// what to do with it. The incumbent is kept, which is the same tie-break
    /// `consensus::chain` uses.
    #[error("header {0} does not extend the most-work chain")]
    NotBest(String),

    /// A proof was checked against a height the client has no header for.
    ///
    /// The whole point of binding a proof to a header: verifying against a root
    /// nobody mined proves nothing at all.
    #[error("no header at height {0}")]
    UnknownHeight(u64),

    /// A proof did not reproduce the state root its header commits to.
    #[error("proof for {address} does not match the state root at height {height}")]
    ProofMismatch {
        /// Hex-encoded account the proof was for.
        address: String,
        /// Height it was checked against.
        height: u64,
    },

    /// The node crate reported a failure while evaluating a proof.
    #[error("{0}")]
    Node(String),
}

impl From<custom_l1_node::error::NodeError> for LightClientError {
    fn from(error: custom_l1_node::error::NodeError) -> Self {
        Self::Node(error.to_string())
    }
}

/// Convenience alias for this crate's fallible operations.
pub type Result<T> = core::result::Result<T, LightClientError>;
