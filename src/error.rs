//! Error types for the node.
//!
//! Every fallible operation returns [`Result`]; no production path panics.

use thiserror::Error;

/// Errors produced by node crypto and consensus primitives.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum NodeError {
    /// Argon2 rejected the requested parameter set.
    #[error("invalid Argon2 parameters: {0}")]
    InvalidArgonParams(String),

    /// The Argon2id pass itself failed.
    #[error("Argon2id hashing failed: {0}")]
    ArgonHash(String),

    /// The public key bytes are not a valid ML-DSA-65 encoding.
    #[error("malformed ML-DSA-65 public key")]
    MalformedPublicKey,

    /// Verification was attempted on a transaction that has not been signed.
    #[error("transaction is not signed")]
    MissingSignature,

    /// The lattice (FIPS 204) signature did not verify against the payload and
    /// public key.
    #[error("ML-DSA-65 signature verification failed")]
    SignatureVerification,

    /// The hash-based (FIPS 205) signature did not verify against the payload
    /// and public key.
    ///
    /// Distinguished from [`NodeError::SignatureVerification`] on purpose.
    /// Every transaction carries both proofs and both must pass, so which half
    /// failed carries real operational meaning: a chain that starts rejecting
    /// one scheme's proofs and not the other's is a chain whose operators need
    /// to know which scheme, immediately.
    #[error("SLH-DSA-SHA2-128s signature verification failed")]
    HashSignatureVerification,

    /// The post-quantum transport handshake failed.
    ///
    /// Distinct from [`NodeError::Network`] because it names a specific,
    /// actionable condition: the peer either does not speak the ML-KEM upgrade
    /// or sent something unusable. An operator seeing these needs to know it is
    /// a protocol mismatch, not a socket problem.
    #[error("ML-KEM transport handshake failed: {0}")]
    PqHandshake(String),

    /// A wire frame received from a peer could not be decoded.
    #[error("malformed wire encoding: {0}")]
    Decode(String),

    /// A transaction was rejected before entering the mempool.
    #[error("transaction rejected: {0}")]
    MempoolRejected(String),

    /// The networking stack reported a failure.
    #[error("network error: {0}")]
    Network(String),

    /// The underlying key-value store reported a failure.
    #[error("storage error: {0}")]
    Storage(String),

    /// The transaction's nonce did not match the sender's expected next nonce.
    ///
    /// This is the condition that rejects a replayed or double-spent
    /// transaction: the first spend advances the nonce, so a second one
    /// carrying the same value no longer matches.
    #[error("invalid nonce for {address}: expected {expected}, got {actual}")]
    InvalidNonce {
        /// Hex-encoded sender address.
        address: String,
        /// Nonce the sender's account currently expects.
        expected: u64,
        /// Nonce the transaction supplied.
        actual: u64,
    },

    /// The sender cannot cover the transaction's outputs.
    #[error("insufficient balance for {address}: need {required}, have {available}")]
    InsufficientBalance {
        /// Hex-encoded sender address.
        address: String,
        /// Total value the transaction attempts to move.
        required: u64,
        /// Balance actually available.
        available: u64,
    },

    /// An arithmetic operation on balances would overflow `u64`.
    #[error("balance arithmetic overflowed")]
    BalanceOverflow,

    /// The computed post-state root did not match the value committed in the
    /// block header.
    #[error("state root mismatch: header commits {expected}, execution produced {actual}")]
    StateRootMismatch {
        /// Hex-encoded root from the block header.
        expected: String,
        /// Hex-encoded root produced by executing the block.
        actual: String,
    },

    /// A channel already exists under the derived identifier.
    #[error("channel {0} already exists")]
    ChannelExists(String),

    /// A channel operation referenced an unknown channel.
    #[error("unknown channel {0}")]
    UnknownChannel(String),

    /// An operation was attempted on a channel in the wrong lifecycle state.
    #[error("channel {channel} is {actual}, expected {expected}")]
    ChannelState {
        /// Hex-encoded channel id.
        channel: String,
        /// Status the channel is actually in.
        actual: &'static str,
        /// Status the operation required.
        expected: &'static str,
    },

    /// A closure's signature did not verify against a registered participant.
    #[error("channel {channel}: signature for party {party} is invalid")]
    ClosureSignature {
        /// Hex-encoded channel id.
        channel: String,
        /// Which side failed: `"a"` or `"b"`.
        party: &'static str,
    },

    /// A closure distributed a different total than the channel holds.
    ///
    /// The check that stops a channel from being used to mint or burn value.
    #[error("channel {channel}: closure totals {actual}, capacity is {expected}")]
    ChannelCapacityMismatch {
        /// Hex-encoded channel id.
        channel: String,
        /// Capacity escrowed on chain.
        expected: u64,
        /// What the closure distributes.
        actual: u64,
    },

    /// A submitted state did not supersede the one already on chain.
    #[error("channel {channel}: state {proposed} does not supersede {current}")]
    StaleChannelState {
        /// Hex-encoded channel id.
        channel: String,
        /// Sequence already recorded.
        current: u64,
        /// Sequence submitted.
        proposed: u64,
    },

    /// A non-participant attempted a channel operation.
    #[error("channel {channel}: {address} is not a participant")]
    NotAParticipant {
        /// Hex-encoded channel id.
        channel: String,
        /// Hex-encoded address that tried.
        address: String,
    },

    /// A penalty was claimed by the same party that submitted the state.
    ///
    /// Self-punishment is not a thing: it would let a closer drain the channel
    /// by "catching" its own fraud.
    #[error("channel {channel}: the closing party cannot claim a penalty")]
    PenaltyByCloser {
        /// Hex-encoded channel id.
        channel: String,
    },

    /// A revocation proof did not match the recorded commitment.
    #[error("channel {channel}: revocation proof does not match the submitted state")]
    InvalidRevocationProof {
        /// Hex-encoded channel id.
        channel: String,
    },

    /// A dispute was finalized before its window elapsed.
    #[error("channel {channel}: dispute window runs until height {deadline}, now {height}")]
    DisputeWindowOpen {
        /// Hex-encoded channel id.
        channel: String,
        /// Height the window closes.
        deadline: u64,
        /// Current height.
        height: u64,
    },

    /// A payload transaction also carried transfer outputs.
    ///
    /// Mixing them would make the value flow ambiguous, so they are disjoint.
    #[error("a {0} transaction must not carry transfer outputs")]
    MixedTransactionKind(&'static str),

    /// A contract already exists at the derived address.
    #[error("contract {0} already exists")]
    ContractExists(String),

    /// A call referenced an address with no deployed code.
    #[error("unknown contract {0}")]
    UnknownContract(String),

    /// The virtual machine refused or failed the execution.
    #[error("vm: {0}")]
    Vm(String),

    /// A shielded proof did not hold against its public inputs.
    #[error("shielded proof rejected: {0}")]
    ProofVerification(String),

    /// A joinsplit named a commitment tree root the pool does not recognise.
    ///
    /// Either the anchor is fabricated, or it aged out of the window while the
    /// transaction sat in a mempool.
    #[error("unknown or expired shielded anchor {anchor}")]
    UnknownAnchor {
        /// Hex of the offending root.
        anchor: String,
    },

    /// A nullifier was already spent, so this is a double-spend.
    #[error("shielded note already spent: nullifier {nullifier}")]
    NullifierSpent {
        /// Hex of the nullifier.
        nullifier: String,
    },

    /// One joinsplit published the same nullifier twice.
    #[error("joinsplit spends the same note twice: nullifier {nullifier}")]
    DuplicateNullifier {
        /// Hex of the repeated nullifier.
        nullifier: String,
    },

    /// The commitment tree reached capacity.
    #[error("the shielded pool is full")]
    ShieldedPoolFull,

    /// More value left the shielded pool than it held.
    ///
    /// Unreachable if the proof system is sound — the circuit enforces value
    /// conservation — so this firing means a soundness break, not a bookkeeping
    /// slip. It is checked precisely because a shielded pool cannot be audited
    /// from outside.
    #[error("shielded pool underflow: holds {held}, {withdrawn} withdrawn")]
    ShieldedBalanceUnderflow {
        /// Value the pool held.
        held: u64,
        /// Value the withdrawal attempted to remove.
        withdrawn: u64,
    },

    /// A block carried more joinsplits than verification budget allows.
    #[error("block has {actual} joinsplits, the limit is {limit}")]
    TooManyShielded {
        /// Joinsplits in the block.
        actual: usize,
        /// Maximum allowed.
        limit: usize,
    },

    /// The shielded setup is untrusted and the network claims to hold value.
    #[error(
        "the shielded pool uses a reproducible test setup, which cannot secure \
         real value on network '{network}'"
    )]
    UntrustedShieldedSetup {
        /// Network the node was asked to serve.
        network: String,
    },

    /// The persisted libp2p identity could not be read, written, or decoded.
    ///
    /// Separate from [`NodeError::Storage`] because the consequence is
    /// different: a node that loses its key does not lose data, it loses the
    /// `PeerId` every bootnode address in the fleet points at.
    #[error("node identity at {path}: {reason}")]
    Identity {
        /// Key file involved.
        path: String,
        /// What went wrong.
        reason: String,
    },

    /// A byte slice had the wrong length to decode into a fixed-size array.
    #[error("invalid {what} length: expected {expected} bytes, got {actual}")]
    InvalidLength {
        /// Name of the field being decoded.
        what: &'static str,
        /// Required length.
        expected: usize,
        /// Length actually supplied.
        actual: usize,
    },
}

/// Convenience alias used throughout the crate.
///
/// Leading `::` disambiguates the `core` crate from this crate's `core` module.
pub type Result<T> = ::core::result::Result<T, NodeError>;
