//! Why a sealed transaction could not be built, opened, or believed.

use core::fmt;

/// A failure in the threshold-encryption path.
///
/// Deliberately coarse about *which* member misbehaved in the arithmetic
/// variants: a caller that learns "share 3 was malformed" from a combine
/// failure learns it from [`MevError::InvalidShareProof`], which names the
/// member, and nowhere else. Encoding failures carry no index because a
/// malformed point tells you nothing about who produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MevError {
    /// A threshold of zero, or one larger than the committee.
    ///
    /// `t = 0` would mean anyone can decrypt with no shares at all; `t > n`
    /// would mean nobody ever can. Both are configuration mistakes that must
    /// fail at construction rather than at the first decryption.
    InvalidThreshold {
        /// Shares required.
        threshold: u16,
        /// Members holding one.
        members: u16,
    },

    /// A committee with no members.
    EmptyCommittee,

    /// A member index outside `1..=members`, or repeated.
    ///
    /// Index `0` is where the secret itself lives in the Shamir polynomial, so
    /// a member holding index `0` would hold the whole key.
    InvalidMemberIndex(u16),

    /// Fewer decryption shares than the threshold requires.
    InsufficientShares {
        /// Shares supplied.
        have: usize,
        /// Shares needed.
        need: u16,
    },

    /// Two shares claiming the same member index.
    ///
    /// Lagrange interpolation divides by the difference of two indices, so a
    /// duplicate is a division by zero — and, less mathematically, a member
    /// voting twice.
    DuplicateShare(u16),

    /// A decryption share carried a proof that does not check out.
    ///
    /// The member is named because this is the one failure where the identity
    /// is the point: a share that fails its Chaum–Pedersen proof is evidence
    /// against a specific committee member, not a transport error.
    InvalidShareProof(u16),

    /// A 32-byte string was not a canonical Ristretto point.
    MalformedPoint,

    /// A 32-byte string was not a canonical scalar.
    MalformedScalar,

    /// A ciphertext was shorter than its own header.
    Truncated,

    /// The AEAD refused to open the ciphertext.
    ///
    /// Means one of: the wrong committee key, a corrupted body, or — the case
    /// that matters — associated data that does not match. A ciphertext sealed
    /// for one height cannot be opened at another, and this is how that shows
    /// up.
    AeadFailure,

    /// The OS refused to supply entropy.
    EntropyFailure,
}

impl fmt::Display for MevError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidThreshold { threshold, members } => write!(
                f,
                "threshold {threshold} is not in 1..={members} for a {members}-member committee"
            ),
            Self::EmptyCommittee => f.write_str("committee has no members"),
            Self::InvalidMemberIndex(index) => {
                write!(f, "member index {index} is out of range or repeated")
            }
            Self::InsufficientShares { have, need } => {
                write!(f, "{have} decryption shares supplied, {need} required")
            }
            Self::DuplicateShare(index) => write!(f, "member {index} supplied two shares"),
            Self::InvalidShareProof(index) => {
                write!(f, "member {index} supplied a share with an invalid proof")
            }
            Self::MalformedPoint => f.write_str("not a canonical ristretto255 point"),
            Self::MalformedScalar => f.write_str("not a canonical scalar"),
            Self::Truncated => f.write_str("ciphertext is shorter than its header"),
            Self::AeadFailure => f.write_str("authenticated decryption failed"),
            Self::EntropyFailure => f.write_str("the operating system refused to supply entropy"),
        }
    }
}

impl core::error::Error for MevError {}

/// Convenience alias for this crate's fallible operations.
pub type Result<T> = core::result::Result<T, MevError>;
