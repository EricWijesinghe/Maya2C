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

/// Fixed-point scale of the adaptive-proportion computation: probabilities
/// and binomial weights are integers in units of 2⁻⁵⁶. The largest product it
/// forms is `w · (W − k) · p` with `w, p ≤ 2⁵⁶` and `W − k ≤ 2⁹`, so 2¹²¹ —
/// inside a `u128`.
const FRAC_BITS: u32 = 56;

/// 1.0 at [`FRAC_BITS`].
const ONE: u128 = 1 << FRAC_BITS;

/// Scale of the square-root chain behind [`two_pow_neg`]: Q62, so a square
/// root `isqrt(x << 62)` and a product of two factors both stay under 2¹²⁴.
const SQRT_BITS: u32 = 62;

/// Bits of the binary expansion of `H mod 1` that [`two_pow_neg`] uses. A
/// declared H is a multiple of 1/1000; truncating its fraction after 30 bits
/// moves `2^−H` by less than one part in 2³⁰, far below what shifts a cutoff.
const EXP_BITS: u32 = 30;

/// Why a source was disabled.
///
/// Carries the counts and the cutoff, never the sample value: a failure is
/// logged and shown to users, and a byte from a key-generation stream is not
/// something either should ever see (Standing Order 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HealthFailure {
    /// `cutoff` identical samples in a row (SP 800-90B §4.4.1).
    #[error("repetition count test: {cutoff} identical samples in a row")]
    RepetitionCount {
        /// The cutoff.
        cutoff: usize,
    },
    /// One value filled `count` of a 512-sample window, at or above `cutoff`
    /// (SP 800-90B §4.4.2).
    #[error(
        "adaptive proportion test: one value {count} times in {APT_WINDOW} samples (cutoff {cutoff})"
    )]
    AdaptiveProportion {
        /// How many times it appeared.
        count: usize,
        /// The cutoff.
        cutoff: usize,
    },
}

/// The repetition-count cutoff for a declared min-entropy, SP 800-90B
/// §4.4.1: `C = 1 + ⌈−log₂ α / H⌉ = 1 + ⌈20 / H⌉`, with H in millibits so
/// the ceiling is an integer division: `1 + ⌈20,000 / millibits⌉`.
#[must_use]
pub fn repetition_cutoff(millibits: u32) -> usize {
    let h = millibits.clamp(1, MAX_MILLIBITS_PER_BYTE);
    let exponent_milli = ALPHA_EXPONENT * 1_000;
    1 + exponent_milli.div_ceil(h) as usize
}

/// `2^(−millibits / 1000)` in units of 2⁻⁵⁶, integer arithmetic only.
///
/// `2^−H = 2^−⌊H⌋ · 2^−f` with `f = H mod 1`. Writing `f` in binary,
/// `f = Σ bᵢ 2⁻ⁱ`, gives `2^−f = Π_{bᵢ = 1} 2^(−2⁻ⁱ)`, and each factor
/// `2^(−2⁻ⁱ)` is the square root of the one before it, starting from
/// `2^(−1/2) = √½`. The bits of `f = r / 1000` come out of long division.
fn two_pow_neg(millibits: u32) -> u128 {
    let (whole, mut rem) = (millibits / 1_000, millibits % 1_000);
    let mut result: u128 = 1 << SQRT_BITS;
    // √½ in Q62: isqrt(2⁶¹ · 2⁶²).
    let mut factor: u128 = (1u128 << (2 * SQRT_BITS - 1)).isqrt();
    for _ in 0..EXP_BITS {
        rem *= 2;
        if rem >= 1_000 {
            rem -= 1_000;
            result = (result * factor) >> SQRT_BITS;
        }
        factor = (factor << SQRT_BITS).isqrt();
    }
    (result >> (SQRT_BITS - FRAC_BITS)) >> whole
}

/// The adaptive-proportion cutoff for a declared min-entropy, W = 512.
///
/// SP 800-90B §4.4.2: `C = 1 + CRITBINOM(W, 2^−H, 1 − α)`, i.e. the smallest
/// `c` with `P[Binomial(W, p) ≥ c] ≤ α`, `p = 2^−H`.
///
/// Integer arithmetic only, so the cutoff is the same on every platform and
/// compiler. The binomial weights are computed relative to the mode
/// `m = ⌊(W + 1) p⌋`, where the weight is largest: `w(m) = 1` and the ratio
/// recurrences `w(k+1) = w(k) · (W−k) p / ((k+1) q)` and
/// `w(k−1) = w(k) · k q / ((W−k+1) p)` fill the rest, so nothing overflows and
/// nothing near the cutoff underflows. Then `P[X ≥ c] ≤ α` is
/// `2²⁰ · Σ_{k≥c} w(k) ≤ Σ_k w(k)`, summed down from `k = W`.
#[must_use]
#[allow(clippy::cast_possible_truncation)] // the mode is ≤ W = 512
pub fn adaptive_proportion_cutoff(millibits: u32) -> usize {
    let p = two_pow_neg(millibits.clamp(1, MAX_MILLIBITS_PER_BYTE));
    let q = ONE - p; // > 0: H ≥ 1 millibit makes p < 1
    let n = APT_WINDOW as u128;
    let mode = (((n + 1) * p) >> FRAC_BITS).min(n) as usize;

    let mut w = [0u128; APT_WINDOW + 1];
    w[mode] = ONE;
    for k in mode..APT_WINDOW {
        let kk = k as u128;
        w[k + 1] = w[k] * (n - kk) * p / ((kk + 1) * q);
    }
    for k in (1..=mode).rev() {
        let kk = k as u128;
        w[k - 1] = w[k] * kk * q / ((n - kk + 1) * p);
    }

    let total: u128 = w.iter().sum();
    let mut tail: u128 = 0;
    for k in (0..=APT_WINDOW).rev() {
        tail += w[k];
        if tail << ALPHA_EXPONENT > total {
            return k + 1;
        }
    }
    // Unreachable: by k = mode the tail holds w(m) = 2⁵⁶, and 2⁵⁶ · 2²⁰
    // exceeds the total, which is at most 513 · 2⁵⁶.
    1
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

    /// §4.4.1's worked example: α = 2⁻²⁰ and H = 2.0 give a repetition
    /// cutoff of `1 + ⌈20 / 2.0⌉ = 11`.
    #[test]
    fn the_repetition_cutoff_matches_the_section_4_4_1_example() {
        assert_eq!(repetition_cutoff(2_000), 11);
    }

    /// The integer `2^−H` is exact where H has a short binary fraction.
    #[test]
    fn two_pow_neg_is_exact_on_dyadic_entropy_claims() {
        assert_eq!(two_pow_neg(8_000), ONE >> 8);
        assert_eq!(two_pow_neg(1_000), ONE >> 1);
        // 2^−0.5 = √½ to within one unit of 2⁻⁵⁶.
        let half_sqrt = (ONE * ONE / 2).isqrt();
        assert!(two_pow_neg(500).abs_diff(half_sqrt) <= 1);
    }

    /// Below a claim of about 0.001 bit the cutoff exceeds the window: the
    /// test cannot fire, which is the formula's answer, not a defect.
    #[test]
    fn a_near_zero_claim_gives_an_unreachable_cutoff() {
        assert_eq!(adaptive_proportion_cutoff(1), APT_WINDOW + 1);
    }

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
            Err(HealthFailure::RepetitionCount { cutoff: 4 })
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
            Err(HealthFailure::AdaptiveProportion { cutoff: 13, .. })
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
