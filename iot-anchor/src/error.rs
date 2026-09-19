//! Failure modes. Decoding and verification only; which `NodeError` a refusal
//! becomes belongs to the caller.

use core::fmt;

/// Why bytes, a signature, or a key source were refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IotError {
    /// Fewer bytes than the layout needs.
    Truncated,
    /// More bytes than the layout describes: a second encoding of one value.
    TrailingBytes,
    /// A sensor class, tamper cause or status tag that names nothing.
    UnknownTag(u8),
    /// Bounds with `min > max`, or a batch with `min > max`, `first > last`, or
    /// more than [`crate::types::MAX_BATCH_READINGS`] readings.
    InvalidRange,
    /// A reading whose counter does not follow the previous one.
    CounterGap,
    /// A public key the ML-DSA-65 decoder refuses.
    BadPublicKey,
    /// A signature that does not verify.
    BadSignature,
    /// Two batches that are not evidence of equivocation.
    NotEquivocation,
    /// The RNG failed while signing.
    SigningFailed,
    /// The PUF response differs from enrollment by more than the code corrects.
    PufUncorrectable,
    /// A stored record no valid sequence of transactions produces.
    NonCanonicalRecord,
    /// The key source (TPM, secure element) failed.
    KeySource,
}

impl fmt::Display for IotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated"),
            Self::TrailingBytes => f.write_str("trailing bytes"),
            Self::UnknownTag(tag) => write!(f, "tag {tag} names nothing"),
            Self::InvalidRange => f.write_str("invalid range"),
            Self::CounterGap => f.write_str("reading counter does not follow the previous one"),
            Self::BadPublicKey => f.write_str("malformed ML-DSA-65 public key"),
            Self::BadSignature => f.write_str("signature does not verify"),
            Self::NotEquivocation => f.write_str("the batches are not conflicting evidence"),
            Self::SigningFailed => f.write_str("signing failed"),
            Self::PufUncorrectable => f.write_str("PUF response beyond the correction budget"),
            Self::NonCanonicalRecord => f.write_str("non-canonical device record"),
            Self::KeySource => f.write_str("key source failed"),
        }
    }
}

impl core::error::Error for IotError {}
