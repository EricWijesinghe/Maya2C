//! The stored record: one per author.

use crate::error::ThreatError;
use crate::evidence::OffenceKind;
use crate::score::{self, MAX_SCORE};

/// Encoded length of an indicator.
pub const INDICATOR_BYTES: usize = 8 + 8 + 8 + 4;

/// What the chain holds about one author.
///
/// No address and no observer. The record says what the author signed and
/// when it was proved, and nothing about who proved it or where the author
/// connects from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThreatIndicator {
    /// Score as of `last_height`, capped at [`MAX_SCORE`].
    pub score: u64,
    /// Height of the block that recorded the first offence.
    pub first_height: u64,
    /// Height of the block that recorded the latest offence.
    pub last_height: u64,
    /// Distinct offences recorded, saturating.
    pub offences: u32,
}

impl ThreatIndicator {
    /// The record after one more verified, distinct offence at `height`.
    ///
    /// Returns a new record; the caller stores it. Distinctness is the
    /// caller's to establish — this crate cannot hash evidence.
    #[must_use]
    pub fn observe(previous: Option<&Self>, kind: OffenceKind, height: u64) -> Self {
        let added = score::weight(kind);
        match previous {
            None => Self {
                score: added.min(MAX_SCORE),
                first_height: height,
                last_height: height,
                offences: 1,
            },
            Some(previous) => Self {
                score: score::decay(previous.score, previous.last_height, height)
                    .saturating_add(added)
                    .min(MAX_SCORE),
                first_height: previous.first_height.min(height),
                last_height: previous.last_height.max(height),
                offences: previous.offences.saturating_add(1),
            },
        }
    }

    /// Whether this record quarantines its author at `height`.
    #[must_use]
    pub const fn is_active(&self, height: u64) -> bool {
        score::is_active(self.score, self.last_height, height)
    }

    /// First height at which the quarantine has lifted.
    #[must_use]
    pub const fn until_height(&self) -> Option<u64> {
        score::until_height(self.score, self.last_height)
    }

    /// The stored form, little-endian.
    #[must_use]
    pub fn encode(&self) -> [u8; INDICATOR_BYTES] {
        let mut bytes = [0u8; INDICATOR_BYTES];
        bytes[..8].copy_from_slice(&self.score.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.first_height.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.last_height.to_le_bytes());
        bytes[24..].copy_from_slice(&self.offences.to_le_bytes());
        bytes
    }

    /// Reads a stored record, refusing any that [`Self::observe`] cannot
    /// produce.
    ///
    /// # Errors
    ///
    /// [`ThreatError::Truncated`], [`ThreatError::TrailingBytes`] or
    /// [`ThreatError::NonCanonicalIndicator`].
    pub fn decode(bytes: &[u8]) -> Result<Self, ThreatError> {
        if bytes.len() < INDICATOR_BYTES {
            return Err(ThreatError::Truncated);
        }
        if bytes.len() > INDICATOR_BYTES {
            return Err(ThreatError::TrailingBytes);
        }
        let u64_at = |at: usize| {
            let mut word = [0u8; 8];
            word.copy_from_slice(&bytes[at..at + 8]);
            u64::from_le_bytes(word)
        };
        let mut offences = [0u8; 4];
        offences.copy_from_slice(&bytes[24..]);
        let record = Self {
            score: u64_at(0),
            first_height: u64_at(8),
            last_height: u64_at(16),
            offences: u32::from_le_bytes(offences),
        };
        if record.offences == 0
            || record.score > MAX_SCORE
            || record.first_height > record.last_height
        {
            return Err(ThreatError::NonCanonicalIndicator);
        }
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::score::{CONFIRM_SCORE, HALF_LIFE_BLOCKS};

    const KIND: OffenceKind = OffenceKind::InvalidSignature;

    #[test]
    fn a_first_offence_confirms_at_once() {
        let record = ThreatIndicator::observe(None, KIND, 40);
        assert_eq!(record.score, CONFIRM_SCORE);
        assert_eq!(record.offences, 1);
        assert!(record.is_active(40));
        assert!(!record.is_active(40 + HALF_LIFE_BLOCKS));
    }

    #[test]
    fn a_repeat_decays_the_old_score_before_adding() {
        let first = ThreatIndicator::observe(None, KIND, 0);
        let second = ThreatIndicator::observe(Some(&first), KIND, HALF_LIFE_BLOCKS);
        assert_eq!(second.score, CONFIRM_SCORE / 2 + CONFIRM_SCORE);
        assert_eq!(second.first_height, 0);
        assert_eq!(second.last_height, HALF_LIFE_BLOCKS);
        assert_eq!(second.offences, 2);
    }

    #[test]
    fn the_score_stops_at_the_cap() {
        let mut record = ThreatIndicator::observe(None, KIND, 1);
        for _ in 0..10_000 {
            record = ThreatIndicator::observe(Some(&record), KIND, 1);
        }
        assert_eq!(record.score, MAX_SCORE);
        assert_eq!(record.offences, 10_001);
    }

    #[test]
    fn a_record_round_trips_and_a_forged_one_does_not_decode() {
        let record = ThreatIndicator::observe(None, KIND, 9);
        assert_eq!(ThreatIndicator::decode(&record.encode()), Ok(record));

        for forged in [
            ThreatIndicator {
                offences: 0,
                ..record
            },
            ThreatIndicator {
                score: MAX_SCORE + 1,
                ..record
            },
            ThreatIndicator {
                first_height: 10,
                ..record
            },
        ] {
            assert_eq!(
                ThreatIndicator::decode(&forged.encode()),
                Err(ThreatError::NonCanonicalIndicator)
            );
        }
        assert_eq!(
            ThreatIndicator::decode(&record.encode()[..27]),
            Err(ThreatError::Truncated)
        );
    }
}
