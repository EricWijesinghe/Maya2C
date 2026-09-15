//! Failure modes of the registry's codecs.
//!
//! Decoding failures only. Whether evidence *verifies* is the node's question,
//! and which `NodeError` a refusal becomes belongs to the caller — the reason
//! `maya-dex` returns its own type too.

use core::fmt;

/// Why bytes are not a canonical attestation or indicator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreatError {
    /// Fewer bytes than the encoding needs.
    Truncated,
    /// More bytes than the encoding describes. A second encoding of one
    /// attestation would be a second transaction id for the same evidence.
    TrailingBytes,
    /// An offence tag that names nothing.
    UnknownOffence(u8),
    /// Evidence larger than [`crate::MAX_EVIDENCE_DATA_BYTES`].
    EvidenceTooLarge {
        /// The declared length.
        len: usize,
    },
    /// An indicator no sequence of observations produces: no offences, a score
    /// above the cap, or a first sighting after the last.
    NonCanonicalIndicator,
}

impl fmt::Display for ThreatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated"),
            Self::TrailingBytes => f.write_str("trailing bytes"),
            Self::UnknownOffence(tag) => write!(f, "offence tag {tag} names nothing"),
            Self::EvidenceTooLarge { len } => write!(
                f,
                "evidence of {len} bytes exceeds {}",
                crate::MAX_EVIDENCE_DATA_BYTES
            ),
            Self::NonCanonicalIndicator => f.write_str("non-canonical indicator"),
        }
    }
}

impl core::error::Error for ThreatError {}
