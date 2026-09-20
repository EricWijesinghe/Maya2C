//! What the disk does to a write that the happy path assumes it does not.
//!
//! A blockchain node's correctness argument leans hard on storage: the undo
//! journal, the block store and the state batch are all "written, therefore
//! durable" (invariants 25, 26, 27). The interesting failures are the ones
//! where that is not true — a write that lands as zeros, a write that lands
//! half, a write that has not landed when the process dies, a write that
//! takes long enough for a timeout to fire.
//!
//! This models the decision, not the bytes: it says *what* the disk did to a
//! write so the caller can act on it. The caller owns the data.

use crate::clock::Duration;
use crate::rng::SimRng;

/// How the simulated disk behaves.
///
/// Defaults are a perfect disk. A test opts into each fault.
#[derive(Clone, Debug)]
pub struct DiskModel {
    /// Lower bound on how long a write takes.
    pub min_write: Duration,
    /// Upper bound on how long a write takes.
    pub max_write: Duration,
    /// Probability that a write takes `slow_write` instead.
    pub slow_ppm: u32,
    /// How long a slow write takes.
    pub slow_write: Duration,
    /// Probability that the bytes that land are not the bytes written.
    pub corrupt_ppm: u32,
    /// Probability that only a prefix of the write lands.
    pub torn_ppm: u32,
    /// Probability that the write is acknowledged but lost on power failure —
    /// the write that makes a crash test interesting.
    pub lost_on_crash_ppm: u32,
    /// Probability that the write fails outright, out of space.
    pub full_ppm: u32,
}

impl Default for DiskModel {
    fn default() -> Self {
        Self {
            min_write: Duration::ZERO,
            max_write: Duration::ZERO,
            slow_ppm: 0,
            slow_write: Duration::from_millis(500),
            corrupt_ppm: 0,
            torn_ppm: 0,
            lost_on_crash_ppm: 0,
            full_ppm: 0,
        }
    }
}

impl DiskModel {
    /// A disk that is slow but honest: 0.1-2 ms, 0.5% of writes taking 500 ms.
    #[must_use]
    pub fn spinning() -> Self {
        Self {
            min_write: Duration::from_micros(100),
            max_write: Duration::from_millis(2),
            slow_ppm: 5_000,
            ..Self::default()
        }
    }

    /// A disk that is failing: slow, and one write in a thousand does
    /// something other than what it was asked.
    #[must_use]
    pub fn failing() -> Self {
        Self {
            corrupt_ppm: 1_000,
            torn_ppm: 1_000,
            lost_on_crash_ppm: 10_000,
            ..Self::spinning()
        }
    }
}

/// What the disk did with one write.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WriteOutcome {
    /// Landed intact after this long.
    Durable(Duration),
    /// Landed, but the bytes are wrong. The caller decides what wrong means;
    /// a checksum is expected to catch this.
    Corrupted(Duration),
    /// Only the first `bytes` of the write landed.
    Torn { after: Duration, bytes: usize },
    /// Acknowledged, then lost because power failed before the flush.
    LostOnCrash(Duration),
    /// Refused: no space.
    Full,
}

impl WriteOutcome {
    /// Whether the caller may treat the data as readable afterwards.
    #[must_use]
    pub const fn landed(self) -> bool {
        matches!(
            self,
            Self::Durable(_) | Self::Corrupted(_) | Self::Torn { .. }
        )
    }

    /// How long the write took. `Full` is immediate.
    #[must_use]
    pub const fn elapsed(self) -> Duration {
        match self {
            Self::Durable(d) | Self::Corrupted(d) | Self::LostOnCrash(d) => d,
            Self::Torn { after, .. } => after,
            Self::Full => Duration::ZERO,
        }
    }
}

/// The simulated disk.
#[derive(Clone, Debug, Default)]
pub struct Disk {
    model: DiskModel,
}

impl Disk {
    /// A perfect disk.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A disk behaving as `model` says.
    #[must_use]
    pub fn with_model(model: DiskModel) -> Self {
        Self { model }
    }

    /// The model in force.
    #[must_use]
    pub fn model(&self) -> &DiskModel {
        &self.model
    }

    /// Replace the model — for a test that degrades a disk part-way through.
    pub fn set_model(&mut self, model: DiskModel) {
        self.model = model;
    }

    /// Decide what happens to a write of `len` bytes.
    ///
    /// Draw order is fixed — full, then duration, then corruption, then
    /// tearing, then crash-loss — because changing it changes the meaning of
    /// every recorded seed.
    pub fn write(&self, len: usize, rng: &mut SimRng) -> WriteOutcome {
        if rng.chance(self.model.full_ppm) {
            return WriteOutcome::Full;
        }
        let lo = self.model.min_write.as_nanos();
        let hi = self.model.max_write.as_nanos().max(lo);
        let mut elapsed = Duration::from_nanos(rng.between(lo, hi));
        if rng.chance(self.model.slow_ppm) {
            elapsed = self.model.slow_write;
        }
        if rng.chance(self.model.corrupt_ppm) {
            return WriteOutcome::Corrupted(elapsed);
        }
        if len > 1 && rng.chance(self.model.torn_ppm) {
            // At least one byte, never the whole write — a "torn" write that
            // happens to be complete is a durable write, and a test that
            // cannot tell them apart is not testing tearing.
            let bytes = rng.between(1, len as u64 - 1) as usize;
            return WriteOutcome::Torn {
                after: elapsed,
                bytes,
            };
        }
        if rng.chance(self.model.lost_on_crash_ppm) {
            return WriteOutcome::LostOnCrash(elapsed);
        }
        WriteOutcome::Durable(elapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::PPM;

    #[test]
    fn a_default_disk_never_lies() {
        let disk = Disk::new();
        let mut rng = SimRng::new(1);
        for _ in 0..5_000 {
            assert_eq!(
                disk.write(4096, &mut rng),
                WriteOutcome::Durable(Duration::ZERO)
            );
        }
    }

    #[test]
    fn a_torn_write_is_always_short_and_never_empty() {
        let disk = Disk::with_model(DiskModel {
            torn_ppm: PPM,
            ..DiskModel::default()
        });
        let mut rng = SimRng::new(2);
        for len in [2usize, 3, 64, 4096] {
            for _ in 0..500 {
                match disk.write(len, &mut rng) {
                    WriteOutcome::Torn { bytes, .. } => {
                        assert!(bytes >= 1 && bytes < len, "{bytes} of {len}");
                    }
                    other => panic!("expected a torn write, got {other:?}"),
                }
            }
        }
    }

    #[test]
    fn a_one_byte_write_cannot_tear() {
        let disk = Disk::with_model(DiskModel {
            torn_ppm: PPM,
            ..DiskModel::default()
        });
        let mut rng = SimRng::new(3);
        for _ in 0..500 {
            assert!(matches!(
                disk.write(1, &mut rng),
                WriteOutcome::Durable(_) | WriteOutcome::LostOnCrash(_)
            ));
        }
    }

    #[test]
    fn a_full_disk_refuses_and_takes_no_time() {
        let disk = Disk::with_model(DiskModel {
            full_ppm: PPM,
            ..DiskModel::default()
        });
        let mut rng = SimRng::new(4);
        let outcome = disk.write(4096, &mut rng);
        assert_eq!(outcome, WriteOutcome::Full);
        assert!(!outcome.landed());
        assert_eq!(outcome.elapsed(), Duration::ZERO);
    }

    #[test]
    fn a_write_lost_on_crash_did_not_land_even_though_it_took_time() {
        let disk = Disk::with_model(DiskModel {
            lost_on_crash_ppm: PPM,
            min_write: Duration::from_millis(1),
            max_write: Duration::from_millis(1),
            ..DiskModel::default()
        });
        let mut rng = SimRng::new(5);
        let outcome = disk.write(4096, &mut rng);
        assert_eq!(outcome, WriteOutcome::LostOnCrash(Duration::from_millis(1)));
        assert!(!outcome.landed(), "a lost write must not read back");
    }

    #[test]
    fn corruption_lands_but_is_not_durable_data() {
        let disk = Disk::with_model(DiskModel {
            corrupt_ppm: PPM,
            ..DiskModel::default()
        });
        let mut rng = SimRng::new(6);
        let outcome = disk.write(4096, &mut rng);
        assert!(matches!(outcome, WriteOutcome::Corrupted(_)));
        assert!(outcome.landed(), "corrupt bytes are still readable bytes");
    }

    #[test]
    fn write_duration_stays_inside_the_bounds_unless_it_is_slow() {
        let disk = Disk::with_model(DiskModel {
            min_write: Duration::from_micros(100),
            max_write: Duration::from_millis(2),
            slow_ppm: 0,
            ..DiskModel::default()
        });
        let mut rng = SimRng::new(7);
        for _ in 0..5_000 {
            let d = disk.write(4096, &mut rng).elapsed().as_nanos();
            assert!((100_000..=2_000_000).contains(&d), "{d}ns outside bounds");
        }
    }

    #[test]
    fn the_same_seed_replays_the_same_outcomes() {
        let disk = Disk::with_model(DiskModel::failing());
        let run = |seed| {
            let mut rng = SimRng::new(seed);
            (0..2_000)
                .map(|_| disk.write(4096, &mut rng))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(31), run(31));
        assert_ne!(run(31), run(32));
    }
}
