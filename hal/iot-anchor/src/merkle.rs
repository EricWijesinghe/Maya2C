//! A heap-free Merkle accumulator over readings, so a microcontroller can
//! commit to a day of readings holding 64 hashes.
//!
//! Peaks are kept per level, as in a Merkle mountain range; the root folds them
//! from the lowest level up. An off-chain verifier that recomputes the root
//! from the archived readings uses this same type.

use crate::error::IotError;
use crate::types::MAX_BATCH_READINGS;
use crate::wire::tagged_hash;

const LEVELS: usize = 64;

// A batch of `MAX_BATCH_READINGS` fills at most `log2` of it plus one levels;
// raising the cap past what 64 peaks can hold would make `push` index out of
// bounds, so the two constants are tied here rather than by a comment.
const _: () = assert!(MAX_BATCH_READINGS.ilog2() < LEVELS as u32);

/// What a device signs about a run of readings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchSummary {
    /// First counter.
    pub first: u64,
    /// Last counter.
    pub last: u64,
    /// Merkle root.
    pub root: [u8; 32],
    /// Lowest reading.
    pub min: i64,
    /// Highest reading.
    pub max: i64,
}

/// Accumulates contiguous readings.
#[derive(Clone, Debug)]
pub struct ReadingsAccumulator {
    peaks: [[u8; 32]; LEVELS],
    occupied: u64,
    first: u64,
    last: u64,
    count: u64,
    min: i64,
    max: i64,
}

impl Default for ReadingsAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

fn leaf(counter: u64, value: i64) -> [u8; 32] {
    tagged_hash(
        b"maya2c iot reading v1",
        &[&counter.to_le_bytes(), &value.to_le_bytes()],
    )
}

fn node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    tagged_hash(b"maya2c iot reading node v1", &[left, right])
}

impl ReadingsAccumulator {
    /// An empty accumulator.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            peaks: [[0; 32]; LEVELS],
            occupied: 0,
            first: 0,
            last: 0,
            count: 0,
            min: i64::MAX,
            max: i64::MIN,
        }
    }

    /// Adds a reading. Counters must be contiguous.
    ///
    /// # Errors
    ///
    /// [`IotError::CounterGap`] for a non-consecutive counter,
    /// [`IotError::InvalidRange`] past [`MAX_BATCH_READINGS`].
    pub fn push(&mut self, counter: u64, value: i64) -> Result<(), IotError> {
        if self.count >= MAX_BATCH_READINGS {
            return Err(IotError::InvalidRange);
        }
        if self.count == 0 {
            self.first = counter;
        } else if self.last.checked_add(1) != Some(counter) {
            return Err(IotError::CounterGap);
        }
        self.last = counter;
        self.count += 1;
        self.min = self.min.min(value);
        self.max = self.max.max(value);

        let mut carry = leaf(counter, value);
        let mut level = 0;
        while self.occupied & (1 << level) != 0 {
            carry = node(&self.peaks[level], &carry);
            self.occupied &= !(1 << level);
            level += 1;
        }
        self.peaks[level] = carry;
        self.occupied |= 1 << level;
        Ok(())
    }

    /// The summary so far, or `None` before the first reading.
    #[must_use]
    pub fn summary(&self) -> Option<BatchSummary> {
        let mut root: Option<[u8; 32]> = None;
        for level in 0..LEVELS {
            if self.occupied & (1 << level) != 0 {
                root = Some(match root {
                    None => self.peaks[level],
                    Some(lower) => node(&self.peaks[level], &lower),
                });
            }
        }
        root.map(|root| BatchSummary {
            first: self.first,
            last: self.last,
            root,
            min: self.min,
            max: self.max,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(first: u64, values: &[i64]) -> [u8; 32] {
        let mut acc = ReadingsAccumulator::new();
        for (offset, value) in (0u64..).zip(values) {
            acc.push(first + offset, *value).expect("push");
        }
        acc.summary().expect("summary").root
    }

    #[test]
    fn every_reading_and_counter_is_committed() {
        let base = root(5, &[1, 2, 3, 4, 5]);
        assert_ne!(base, root(5, &[1, 2, 3, 4, 6]));
        assert_ne!(base, root(6, &[1, 2, 3, 4, 5]));
        assert_ne!(base, root(5, &[1, 2, 3, 4]));
        assert_eq!(base, root(5, &[1, 2, 3, 4, 5]));
    }

    #[test]
    fn gaps_and_empty_accumulators_are_refused() {
        let mut acc = ReadingsAccumulator::new();
        assert_eq!(acc.summary(), None);
        acc.push(u64::MAX, 1).expect("first");
        assert_eq!(acc.push(0, 1), Err(IotError::CounterGap));
        let summary = acc.summary().expect("one reading");
        assert_eq!(
            (summary.first, summary.last, summary.min, summary.max),
            (u64::MAX, u64::MAX, 1, 1)
        );
    }
}
