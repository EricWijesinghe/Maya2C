//! Every way a ceremony can refuse.
//!
//! The variants are deliberately specific about *which* custodian and *which*
//! index, because the operational question after a failed ceremony is never
//! "did it fail" — it is "who do I call". A single `InvalidShare` would satisfy
//! the type checker and satisfy nobody at 3am.

use thiserror::Error;

/// The result type every fallible operation in this crate returns.
pub type Result<T> = core::result::Result<T, CustodyError>;

/// A refusal, named precisely enough to act on.
#[derive(Debug, Error, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum CustodyError {
    /// A vault with no custodians, which is a vault with no key.
    #[error("a vault needs at least one custodian")]
    EmptyVault,

    /// `threshold` outside `1..=custodians`.
    ///
    /// A threshold above the roster can never be met; a threshold of zero means
    /// the empty set reconstructs.
    #[error("threshold {threshold} is not in 1..={custodians}")]
    InvalidThreshold {
        /// The threshold asked for.
        threshold: u8,
        /// The roster size it was asked against.
        custodians: u8,
    },

    /// Index `0` is the secret's own position on the polynomial, not a
    /// custodian's.
    ///
    /// A custodian issued index `0` would be issued the whole key. This is the
    /// kind of off-by-one that returns a correct answer to the wrong question.
    #[error("custodian index 0 is the secret's own position, not a custodian's")]
    ReservedIndex,

    /// An index above the roster size.
    #[error("custodian index {index} is outside the roster of {custodians}")]
    UnknownCustodian {
        /// The offending index.
        index: u8,
        /// The roster size.
        custodians: u8,
    },

    /// The same custodian contributed twice.
    ///
    /// Two shares at one index interpolate through a zero denominator, so this
    /// is a division by zero before it is a policy violation — and a quorum of
    /// "three shares" that is really one custodian three times is not a quorum.
    #[error("custodian {0} contributed twice")]
    DuplicateContribution(u8),

    /// Fewer contributions than the threshold.
    #[error("{received} of {threshold} required contributions")]
    ShortOfThreshold {
        /// How many arrived.
        received: usize,
        /// How many the vault requires.
        threshold: u8,
    },

    /// A share does not satisfy the dealer's published commitment.
    ///
    /// This names the dealer, not the recipient: the share arrived sealed to
    /// the recipient, so a share that fails verification is the dealer's doing.
    #[error("custodian {dealer} dealt custodian {recipient} a share its own commitment rejects")]
    InconsistentShare {
        /// Who dealt it.
        dealer: u8,
        /// Who received it.
        recipient: u8,
    },

    /// A dealer published a commitment vector of the wrong degree.
    #[error("custodian {dealer} committed to {found} coefficients, expected {expected}")]
    MalformedCommitment {
        /// Who published it.
        dealer: u8,
        /// How many coefficients arrived.
        found: usize,
        /// How many the threshold requires.
        expected: usize,
    },

    /// A dealer is missing from a round that requires everybody.
    ///
    /// Distribution is all-or-nothing: a vault built from a subset of the
    /// roster's contributions is a vault whose absent members hold shares of a
    /// different key.
    #[error("custodian {0} did not contribute to distribution")]
    MissingDealer(u8),

    /// A point on the curve that is not a valid Ristretto encoding.
    #[error("custodian {0} sent a malformed commitment point")]
    MalformedPoint(u8),

    /// A sealed share did not open.
    ///
    /// Either the KEM ciphertext was not for this recipient or the AEAD tag
    /// failed. The two are deliberately not distinguished: which one it was is
    /// information an attacker would like and an operator cannot use.
    #[error("the sealed share from custodian {0} did not open")]
    SealFailed(u8),

    /// A message that belongs to a different vault.
    ///
    /// The ceremony id is bound into every commitment and every sealed share
    /// precisely so that a share from last quarter's vault is rejected rather
    /// than mixed into this quarter's.
    #[error("message belongs to a different vault")]
    WrongVault,

    /// The reconstructed seed did not derive the vault's recorded address.
    ///
    /// The one check that catches a short quorum, a corrupted share store, and
    /// a dealer whose shares were not all on one polynomial — none of which the
    /// arithmetic itself can detect, because interpolating too few points
    /// yields a *wrong* answer rather than an error.
    #[error("the quorum reconstructed a seed for a different address")]
    WrongSeed,

    /// The OS entropy source was unavailable.
    ///
    /// Not recoverable by retrying, and never to be papered over with a
    /// fallback: a predictable contribution makes a predictable vault key.
    #[error("the OS entropy source is unavailable")]
    EntropyFailure,

    /// ML-DSA key generation or signing failed.
    #[error("lattice signature operation failed: {0}")]
    Lattice(&'static str),

    /// A message claimed a custodian index that does not belong to the
    /// authenticated peer it arrived from.
    ///
    /// Nothing inside a dealing or a contribution proves who sent it — a share
    /// is sealed *to* its recipient, not signed by its dealer — so the index is
    /// only as trustworthy as the connection's identity. Without this check an
    /// authenticated custodian can submit a dealing labelled as someone else's,
    /// take their slot, and have the real one refused as a duplicate.
    #[error("custodian {claimed} is not who this connection authenticated as")]
    ImpersonatedCustodian {
        /// The index the message claimed.
        claimed: u8,
    },

    /// A wire message that does not decode.
    #[error("malformed wire message: {0}")]
    Malformed(&'static str),

    /// Threshold lattice signing was asked for, and no scheme has been chosen
    /// in an ADR (ADR-011). Not a transient failure.
    #[error("no threshold lattice signing scheme has been chosen (ADR-011)")]
    NoThresholdScheme,
}
