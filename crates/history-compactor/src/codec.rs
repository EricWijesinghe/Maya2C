//! Wire form of an [`InclusionProof`]: it arrives from an archive, so the
//! decoder bounds every length before it allocates.
//!
//! `leaf_index u64 LE | leaves u64 LE | siblings u8 | peaks u8 | hashes…`

use crate::InclusionProof;

/// Fixed header bytes before the hashes.
pub(crate) const HEADER_LEN: usize = 8 + 8 + 1 + 1;
/// A range over `u64` leaves has at most 64 levels and 64 peaks.
const MAX_HASHES: usize = 64;

/// Why proof bytes were refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Fewer bytes than the header or the declared hashes need.
    Truncated,
    /// Bytes left over after the declared hashes.
    TrailingBytes,
    /// A count above what any `u64`-leaf range can have.
    TooManyHashes,
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Truncated => "proof is truncated",
            Self::TrailingBytes => "proof has trailing bytes",
            Self::TooManyHashes => "proof declares more hashes than any range has",
        })
    }
}

impl std::error::Error for DecodeError {}

impl InclusionProof {
    /// Canonical bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.encoded_len());
        out.extend_from_slice(&self.leaf_index.to_le_bytes());
        out.extend_from_slice(&self.leaves.to_le_bytes());
        // Both lengths are ≤ 64 for any proof `ArchiveMmr::prove` builds.
        out.push(u8::try_from(self.siblings.len()).unwrap_or(u8::MAX));
        out.push(u8::try_from(self.other_peaks.len()).unwrap_or(u8::MAX));
        for h in self.siblings.iter().chain(&self.other_peaks) {
            out.extend_from_slice(h);
        }
        out
    }

    /// Parses bytes from an untrusted archive. Shape is checked here; whether
    /// the hashes are right is [`InclusionProof::verify`]'s job.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (header, body) = bytes.split_at_checked(HEADER_LEN).ok_or(DecodeError::Truncated)?;
        let u64_at = |at: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&header[at..at + 8]);
            u64::from_le_bytes(b)
        };
        let siblings = usize::from(header[16]);
        let peaks = usize::from(header[17]);
        if siblings > MAX_HASHES || peaks > MAX_HASHES {
            return Err(DecodeError::TooManyHashes);
        }
        let need = 32 * (siblings + peaks);
        match body.len().cmp(&need) {
            core::cmp::Ordering::Less => return Err(DecodeError::Truncated),
            core::cmp::Ordering::Greater => return Err(DecodeError::TrailingBytes),
            core::cmp::Ordering::Equal => {}
        }
        let mut hashes = body.as_chunks::<32>().0.iter().copied();
        Ok(Self {
            leaf_index: u64_at(0),
            leaves: u64_at(8),
            siblings: hashes.by_ref().take(siblings).collect(),
            other_peaks: hashes.collect(),
        })
    }
}
