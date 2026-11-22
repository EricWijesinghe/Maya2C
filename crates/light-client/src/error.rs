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

    /// A block broke a rule its witness let the client check.
    #[error("block at height {height} is invalid: {reason}")]
    BlockInvalid {
        /// Height of the block.
        height: u64,
        /// What it broke.
        reason: String,
    },

    /// A block could not be decided from the witness and body offered.
    ///
    /// Not a verdict on the block: a witness travels outside the block's id and
    /// proof of work, so a relay can corrupt it. Fetch again.
    #[error("block at height {height} is not verifiable from this witness: {reason}")]
    BlockUnverifiable {
        /// Height of the block.
        height: u64,
        /// Why not.
        reason: String,
    },

    /// A checkpoint's quorum, or one of its signatures, did not verify under
    /// the trusted committee: short of quorum, forged, or signed for another
    /// chain. The node's own verifier said why; the reason is carried through.
    #[error("checkpoint rejected: {0}")]
    CheckpointRejected(String),

    /// A checkpoint names an epoch other than the one this client trusts.
    #[error("checkpoint is for epoch {got}; the trusted committee is epoch {expected}")]
    WrongEpoch {
        /// The epoch this client trusts.
        expected: u64,
        /// The epoch the checkpoint names.
        got: u64,
    },

    /// A checkpoint's block is not the block its header hashes to.
    #[error("header does not hash to the checkpoint block at height {0}")]
    HeaderMismatch(u64),

    /// A checkpoint is below the trusted period, or below one already
    /// accepted. A chain's height is its clock, so "old" is counted
    /// in blocks.
    #[error("checkpoint at height {height} is below the trusted floor {floor}")]
    StaleCheckpoint {
        /// Height of the checkpoint offered.
        height: u64,
        /// The lowest height this client will accept.
        floor: u64,
    },

    /// An epoch handoff's next committee could not be proven against the
    /// state root of its header.
    #[error("committee handoff rejected: {0}")]
    RotationRejected(String),

    /// A trust policy or anchor that cannot be honest, such as a trusted
    /// period as long as the unbonding period.
    #[error("trust configuration rejected: {0}")]
    TrustConfig(String),

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
