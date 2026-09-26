//! SP 800-90B §4.4 continuous health tests.
//!
//! Both run on every sample a source produces, with the false-positive rate
//! the standard recommends, α = 2⁻²⁰, and cutoffs computed from the source's
//! *declared* min-entropy `H` (bits per one-byte sample):
//!
//! - **Repetition count** (§4.4.1): a run of `C = 1 + ⌈20 / H⌉` identical
//!   samples fails. Catches a source that has stuck.
//! - **Adaptive proportion** (§4.4.2): in each window of `W = 512` samples, the
//!   first sample occurring `C` times or more fails, where
//!   `C = 1 + CRITBINOM(W, 2^−H, 1 − α)`. Catches a source that has become
//!   biased without sticking.
//!
//! The cutoffs match SP 800-90B Table 2 for W = 512 (H = 0.5, 1, 2, 4, 8 →
//! 410, 311, 177, 62, 13); the tests below pin those values.
//!
//! These are *continuous* tests: they catch a source that breaks, not one
//! that was never random. Whether a source's output is random at all is what
//! the offline batteries (SP 800-22, Dieharder) are for.

use crate::MAX_MILLIBITS_PER_BYTE;

/// The adaptive-proportion window for non-binary sources, SP 800-90B §4.4.2.
pub const APT_WINDOW: usize = 512;

/// −log₂ α, where α = 2⁻²⁰ is the false-positive probability per test.
const ALPHA_EXPONENT: u32 = 20;

/// Why a source was disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthFailure {
    /// `run` identical samples in a row, at or above `cutoff`.
    RepetitionCount {
        /// The repeated value.
        value: u8,
        /// The cutoff.
        cutoff: usize,
    },
    /// One value filled `count` of a 512-sample window, at or above `cutoff`.
    AdaptiveProportion {
        /// The over-represented value.
        value: u8,
        /// How many times it appeared.
        count: usize,
        /// The cutoff.
        cutoff: usize,
    },
}

/// The repetition-count cutoff for a declared min-entropy.
#[must_use]
pub fn repetition_cutoff(millibits: u32) -> usize {
    let h = millibits.clamp(1, MAX_MILLIBITS_PER_BYTE);
    let exponent_milli = ALPHA_EXPONENT * 1_000;
    1 + exponent_milli.div_ceil(h) as usize
}

/// The adaptive-proportion cutoff for a declared min-entropy, W = 512.
///
/// The smallest `c` with `P[Binomial(W, 2^−H) ≥ c] ≤ α`, found by summing
/// the binomial distribution in log space. Floating point is acceptable here:
/// this is a threshold for a local health alarm, never a consensus rule.
#[must_use]
#[allow(clippy::cast_precision_loss)] // k ≤ 512: exact in an f64
pub fn adaptive_proportion_cutoff(millibits: u32) -> usize {
    let h = f64::from(millibits.clamp(1, MAX_MILLIBITS_PER_BYTE)) / 1_000.0;
    let p = (-h).exp2();
    let target = 1.0 - (-f64::from(ALPHA_EXPONENT)).exp2();
    let n = APT_WINDOW as f64;
    let (ln_p, ln_q) = (p.ln(), (1.0 - p).ln());

    let mut ln_choose = 0.0; // ln C(W, 0)
    let mut cdf = 0.0;
    for k in 0..=APT_WINDOW {
        let kf = k as f64;
        if k > 0 {
            ln_choose += ((n - kf + 1.0) / kf).ln();
        }
        cdf += (ln_choose + kf * ln_p + (n - kf) * ln_q).exp();
        if cdf >= target {
            return 1 + k;
        }
    }
    APT_WINDOW
}

/// Both tests over one source's sample stream.
#[derive(Debug, Clone)]
pub struct HealthMonitor {
    rct_cutoff: usize,
    apt_cutoff: usize,
    last: Option<u8>,
    run: usize,
    window_first: Option<u8>,
    window_seen: usize,
    window_count: usize,
    failure: Option<HealthFailure>,
    samples: u64,
}

impl HealthMonitor {
    /// A monitor for a source claiming `millibits` of min-entropy per byte.
    #[must_use]
    pub fn new(millibits: u32) -> Self {
        Self {
            rct_cutoff: repetition_cutoff(millibits),
            apt_cutoff: adaptive_proportion_cutoff(millibits),
            last: None,
            run: 0,
            window_first: None,
            window_seen: 0,
            window_count: 0,
            failure: None,
            samples: 0,
        }
    }

    /// The first failure seen, if any. Latched: once failed, always failed.
    #[must_use]
    pub fn failure(&self) -> Option<HealthFailure> {
        self.failure
    }

    /// Samples examined so far.
    #[must_use]
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Feeds samples through both tests.
    ///
    /// # Errors
    ///
    /// The latched failure, if this or any earlier sample tripped a test.
    pub fn check(&mut self, samples: &[u8]) -> Result<(), HealthFailure> {
        for &sample in samples {
            if let Some(failure) = self.failure {
                return Err(failure);
            }
            self.samples += 1;
            self.repetition(sample);
            self.adaptive(sample);
        }
        self.failure.map_or(Ok(()), Err)
    }

    fn repetition(&mut self, sample: u8) {
        if self.last == Some(sample) {
            self.run += 1;
        } else {
            self.last = Some(sample);
            self.run = 1;
        }
        if self.run >= self.rct_cutoff {
            self.failure.get_or_insert(HealthFailure::RepetitionCount {
                value: sample,
                cutoff: self.rct_cutoff,
            });
        }
    }

    fn adaptive(&mut self, sample: u8) {
        let Some(first) = self.window_first else {
            self.window_first = Some(sample);
            self.window_seen = 1;
            self.window_count = 1;
            return;
        };
        self.window_seen += 1;
        if sample == first {
            self.window_count += 1;
            if self.window_count >= self.apt_cutoff {
                self.failure
                    .get_or_insert(HealthFailure::AdaptiveProportion {
                        value: first,
                        count: self.window_count,
                        cutoff: self.apt_cutoff,
                    });
            }
        }
        if self.window_seen == APT_WINDOW {
            self.window_first = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cutoffs_match_sp_800_90b_table_2() {
        let table = [
            (500, 41, 410),
            (1_000, 21, 311),
            (2_000, 11, 177),
            (4_000, 6, 62),
            (8_000, 4, 13),
        ];
        for (millibits, rct, apt) in table {
            assert_eq!(
                repetition_cutoff(millibits),
                rct,
                "RCT at H = {millibits} mbit"
            );
            assert_eq!(
                adaptive_proportion_cutoff(millibits),
                apt,
                "APT at H = {millibits} mbit"
            );
        }
    }

    #[test]
    fn a_stuck_source_fails_the_repetition_count_test() {
        let mut monitor = HealthMonitor::new(8_000);
        assert_eq!(monitor.check(&[1, 2, 3]), Ok(()));
        assert_eq!(
            monitor.check(&[9, 9, 9, 9]),
            Err(HealthFailure::RepetitionCount {
                value: 9,
                cutoff: 4
            })
        );
        // Latched.
        assert!(monitor.check(&[1, 2, 3]).is_err());
    }

    #[test]
    fn a_biased_source_fails_the_adaptive_proportion_test() {
        let mut monitor = HealthMonitor::new(8_000);
        // Alternate 7 with a counter so no run repeats, but 7 is half the window.
        let biased: Vec<u8> = (0..512u32)
            .map(|i| if i % 2 == 0 { 7 } else { (i % 251) as u8 + 1 })
            .collect();
        assert!(matches!(
            monitor.check(&biased),
            Err(HealthFailure::AdaptiveProportion {
                value: 7,
                cutoff: 13,
                ..
            })
        ));
    }

    /// A fixed stream, not the OS source: at H = 8 the repetition cutoff is 4,
    /// and four equal bytes in a row occur with probability 2⁻²⁴ per position,
    /// so a fresh 2²⁰-byte OS draw trips it about 1 run in 16 — α = 2⁻²⁰ per
    /// sample is the standard's accepted false-alarm rate, not a defect. A
    /// deterministic SHA-256 counter stream passes or fails the same way on
    /// every run.
    #[test]
    fn a_good_source_passes_a_million_samples() {
        use sha2::{Digest, Sha256};
        let buf: Vec<u8> = (0u64..(1 << 15))
            .flat_map(|i| Sha256::digest(i.to_le_bytes()))
            .collect();
        let mut monitor = HealthMonitor::new(8_000);
        assert_eq!(monitor.check(&buf), Ok(()));
        assert_eq!(monitor.samples(), 1 << 20);
    }

    /// The false-alarm rate above, measured: of 64 independent 64 KiB OS
    /// draws, the expected number that trip is 64 × 2¹⁶ × 2⁻²⁴ = 0.25, so a
    /// monitor that trips on most of them is miscalibrated.
    #[test]
    fn os_entropy_trips_only_at_the_standards_rate() {
        let tripped = (0..64)
            .filter(|_| {
                let mut buf = vec![0u8; 1 << 16];
                getrandom::fill(&mut buf).expect("os entropy");
                HealthMonitor::new(8_000).check(&buf).is_err()
            })
            .count();
        assert!(tripped <= 8, "{tripped} of 64 draws tripped");
    }
}
