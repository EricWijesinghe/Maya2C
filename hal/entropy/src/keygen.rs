//! Health-checked entropy for key generation — ADR-137 HW-5.
//!
//! Wallets and key tools call [`KeygenEntropy::os`] and take their seed from
//! [`KeygenEntropy::seed`] instead of calling `getrandom` themselves. Between
//! the two, the bytes pass SP 800-90B's health tests:
//!
//! - **start-up** (§4.3, requirement 2 of §4.4): [`STARTUP_SAMPLES`] samples
//!   run through both continuous tests before the first byte is released.
//!   They are discarded and zeroized, never used as key material;
//! - **continuous** (§4.4.1, §4.4.2): every byte handed out afterwards goes
//!   through the same monitor — the repetition count and adaptive proportion
//!   tests in [`crate::health`] — *before* it is handed out.
//!
//! A failure is latched and fails closed: the caller gets a
//! [`KeygenEntropyError`], the output buffer is zeroed, and there is no
//! fallback source. The OS generator is already conditioned, so these tests
//! catch a broken or stubbed generator (all zeros, a stuck constant, a gross
//! bias), not a subtle weakness; that is what they are for.
//!
//! [`HealthStatus`] is what a UI may show: counts and the last failure,
//! never a sample value.

use zeroize::{Zeroize, Zeroizing};

use crate::health::{HealthFailure, HealthMonitor};
use crate::sources::OsSource;
use crate::{EntropyError, EntropySource};

/// Samples tested at start-up before any output, SP 800-90B §4.3. The
/// standard's floor is 1,024; 4,096 covers eight full adaptive-proportion
/// windows of 512.
pub const STARTUP_SAMPLES: usize = 4_096;

/// Start-up attempts before key generation is refused: one retry, on fresh
/// samples and a fresh monitor.
///
/// A healthy H = 8 source fails one 4,096-sample start-up with probability
/// 2.53 × 10⁻⁴, about 1 in 3,956 (exact, computed from the cutoffs in
/// [`crate::health`]: repetition count, cutoff 4, about 1 in 4,116; adaptive
/// proportion, cutoff 13, 1.22 × 10⁻⁶ per window × 8 windows, about 1 in
/// 102,303). The integer cutoffs put the repetition count's real rate at
/// 2⁻²⁴ per sample, below the α = 2⁻²⁰ bound. Two independent failures:
/// about 6.4 × 10⁻⁸, 1 in 15.7 million. A stuck or biased source fails both.
pub const STARTUP_ATTEMPTS: u32 = 2;

/// Why key generation refused to proceed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeygenEntropyError {
    /// The start-up test failed; no output was ever released.
    #[error("entropy start-up test failed: {0}")]
    Startup(HealthFailure),
    /// A continuous test failed on bytes requested for a key.
    #[error("entropy health test failed: {0}")]
    Health(HealthFailure),
    /// The source itself reported an error.
    #[error(transparent)]
    Source(#[from] EntropyError),
}

/// A source's health, for a status line. Never contains a sample value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HealthStatus {
    /// Samples run through the live monitor, start-up included (a failed
    /// start-up attempt's samples are not counted).
    pub samples_tested: u64,
    /// Whether the start-up test passed.
    pub startup_passed: bool,
    /// Start-up attempts made, at most [`STARTUP_ATTEMPTS`].
    pub startup_attempts: u32,
    /// Requests refused because of a health failure.
    pub failures: u64,
    /// The latched failure, if any.
    pub last_failure: Option<HealthFailure>,
}

/// An entropy source behind start-up and continuous health tests.
pub struct KeygenEntropy<S = OsSource> {
    source: S,
    monitor: HealthMonitor,
    status: HealthStatus,
}

impl<S> core::fmt::Debug for KeygenEntropy<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("KeygenEntropy")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl KeygenEntropy<OsSource> {
    /// The operating system's generator, after a passing start-up test.
    ///
    /// # Errors
    ///
    /// [`KeygenEntropyError::Startup`] if the start-up test fails, or
    /// [`KeygenEntropyError::Source`] if the OS call fails.
    pub fn os() -> Result<Self, KeygenEntropyError> {
        Self::new(OsSource)
    }
}

impl<S: EntropySource> KeygenEntropy<S> {
    /// Wraps `source` and runs the start-up test on [`STARTUP_SAMPLES`] of
    /// its output, which are then zeroized and dropped. A failed attempt is
    /// retried once on fresh samples ([`STARTUP_ATTEMPTS`]).
    ///
    /// # Errors
    ///
    /// [`KeygenEntropyError::Startup`] with the last attempt's failure, or
    /// [`KeygenEntropyError::Source`] (not retried).
    pub fn new(mut source: S) -> Result<Self, KeygenEntropyError> {
        let mut startup = Zeroizing::new(vec![0u8; STARTUP_SAMPLES]);
        let mut attempt = 1;
        loop {
            let mut monitor = HealthMonitor::new(source.min_entropy_millibits());
            source.fill(&mut startup)?;
            let checked = monitor.check(&startup);
            // `[u8]::zeroize`, not `Vec::zeroize`: the latter also truncates.
            startup.as_mut_slice().zeroize();
            match checked {
                Ok(()) => {
                    let status = HealthStatus {
                        samples_tested: monitor.samples(),
                        startup_passed: true,
                        startup_attempts: attempt,
                        failures: 0,
                        last_failure: None,
                    };
                    return Ok(Self {
                        source,
                        monitor,
                        status,
                    });
                }
                Err(failure) if attempt < STARTUP_ATTEMPTS => {
                    // The failure's Display carries counts and cutoffs only.
                    tracing::warn!(
                        attempt,
                        max = STARTUP_ATTEMPTS,
                        %failure,
                        "entropy start-up test failed; retrying on fresh samples"
                    );
                    attempt += 1;
                }
                Err(failure) => return Err(KeygenEntropyError::Startup(failure)),
            }
        }
    }

    /// Fills `out` with source bytes that passed the continuous tests.
    ///
    /// # Errors
    ///
    /// [`KeygenEntropyError::Health`] once any test has failed (latched), or
    /// [`KeygenEntropyError::Source`]. On error `out` is zeroed: there is
    /// never partial or untested output.
    pub fn fill(&mut self, out: &mut [u8]) -> Result<(), KeygenEntropyError> {
        let result = self.fill_inner(out);
        self.status.samples_tested = self.monitor.samples();
        if let Err(KeygenEntropyError::Health(failure)) = &result {
            self.status.failures += 1;
            self.status.last_failure = Some(*failure);
        }
        if result.is_err() {
            out.zeroize();
        }
        result
    }

    fn fill_inner(&mut self, out: &mut [u8]) -> Result<(), KeygenEntropyError> {
        if let Some(failure) = self.monitor.failure() {
            return Err(KeygenEntropyError::Health(failure));
        }
        self.source.fill(out)?;
        self.monitor.check(out).map_err(KeygenEntropyError::Health)
    }

    /// An `N`-byte seed that passed the continuous tests, zeroized on drop.
    ///
    /// # Errors
    ///
    /// As [`Self::fill`].
    pub fn seed<const N: usize>(&mut self) -> Result<Zeroizing<[u8; N]>, KeygenEntropyError> {
        let mut seed = Zeroizing::new([0u8; N]);
        self.fill(seed.as_mut())?;
        Ok(seed)
    }

    /// Counts and the last failure, safe to log and display.
    #[must_use]
    pub fn status(&self) -> HealthStatus {
        self.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SourceClass;
    use sha2::{Digest, Sha256};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A scripted source: a deterministic stream, so a pass or a fail is the
    /// same on every run (an OS draw trips the RCT at α = 2⁻²⁰ per sample).
    struct Scripted {
        stream: Vec<u8>,
        at: usize,
        millibits: u32,
    }

    impl Scripted {
        fn new(stream: Vec<u8>, millibits: u32) -> Self {
            Self {
                stream,
                at: 0,
                millibits,
            }
        }

        /// A SHA-256 counter stream: passes both tests at H = 8.
        fn good(len: usize) -> Self {
            let stream = (0u64..)
                .flat_map(|i| Sha256::digest(i.to_le_bytes()))
                .take(len)
                .collect();
            Self::new(stream, 8_000)
        }
    }

    impl EntropySource for Scripted {
        fn name(&self) -> &'static str {
            "scripted"
        }
        fn class(&self) -> SourceClass {
            SourceClass::Real
        }
        fn min_entropy_millibits(&self) -> u32 {
            self.millibits
        }
        fn fill(&mut self, out: &mut [u8]) -> Result<(), EntropyError> {
            let end = self.at + out.len();
            let chunk = self
                .stream
                .get(self.at..end)
                .ok_or_else(|| EntropyError::SourceFailed("scripted: exhausted".into()))?;
            out.copy_from_slice(chunk);
            self.at = end;
            Ok(())
        }
    }

    /// Counts `fill` calls, observable after the source moves into `new`.
    struct Counted {
        inner: Scripted,
        fills: Arc<AtomicUsize>,
    }

    impl EntropySource for Counted {
        fn name(&self) -> &'static str {
            "counted"
        }
        fn class(&self) -> SourceClass {
            SourceClass::Real
        }
        fn min_entropy_millibits(&self) -> u32 {
            self.inner.millibits
        }
        fn fill(&mut self, out: &mut [u8]) -> Result<(), EntropyError> {
            self.fills.fetch_add(1, Ordering::SeqCst);
            self.inner.fill(out)
        }
    }

    fn counted(stream: Vec<u8>) -> (Counted, Arc<AtomicUsize>) {
        let fills = Arc::new(AtomicUsize::new(0));
        let source = Counted {
            inner: Scripted::new(stream, 8_000),
            fills: Arc::clone(&fills),
        };
        (source, fills)
    }

    #[test]
    fn a_transient_startup_failure_then_a_good_source_succeeds() {
        let mut stream = vec![0u8; STARTUP_SAMPLES];
        stream.extend(Scripted::good(STARTUP_SAMPLES + 32).stream);
        let (source, fills) = counted(stream);
        let mut entropy = KeygenEntropy::new(source).expect("retry passes");
        let status = entropy.status();
        assert!(status.startup_passed);
        assert_eq!(status.startup_attempts, 2);
        assert_eq!(status.samples_tested, STARTUP_SAMPLES as u64);
        assert_eq!(fills.load(Ordering::SeqCst), 2);
        entropy.seed::<32>().expect("seed after retry");
    }

    #[test]
    fn a_stuck_source_fails_after_two_startup_attempts() {
        let (source, fills) = counted(vec![0u8; 3 * STARTUP_SAMPLES]);
        assert_eq!(
            KeygenEntropy::new(source).err(),
            Some(KeygenEntropyError::Startup(
                HealthFailure::RepetitionCount { cutoff: 4 }
            ))
        );
        assert_eq!(fills.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn startup_never_makes_more_than_two_attempts() {
        // A third attempt would pass; it must never be made.
        let mut stream = vec![0u8; 2 * STARTUP_SAMPLES];
        stream.extend(Scripted::good(STARTUP_SAMPLES).stream);
        let (source, fills) = counted(stream);
        assert!(matches!(
            KeygenEntropy::new(source),
            Err(KeygenEntropyError::Startup(_))
        ));
        assert_eq!(fills.load(Ordering::SeqCst), 2);
        assert_eq!(STARTUP_ATTEMPTS, 2);
    }

    #[test]
    fn a_stuck_all_zero_source_fails_the_startup_repetition_test() {
        let stuck = Scripted::new(vec![0u8; 2 * STARTUP_SAMPLES], 8_000);
        assert_eq!(
            KeygenEntropy::new(stuck).err(),
            Some(KeygenEntropyError::Startup(
                HealthFailure::RepetitionCount { cutoff: 4 }
            ))
        );
    }

    #[test]
    fn a_biased_source_fails_the_startup_adaptive_proportion_test() {
        // 0x55 in every other position, a counter elsewhere: no run longer
        // than one, but half of every 512-sample window is one value.
        let biased = (0..2 * STARTUP_SAMPLES)
            .map(|i| {
                if i % 2 == 0 {
                    0x55
                } else {
                    (i % 200) as u8 + 1
                }
            })
            .collect();
        let err = KeygenEntropy::new(Scripted::new(biased, 8_000)).err();
        assert!(
            matches!(
                err,
                Some(KeygenEntropyError::Startup(
                    HealthFailure::AdaptiveProportion { cutoff: 13, .. }
                ))
            ),
            "{err:?}"
        );
    }

    #[test]
    fn a_good_source_passes_startup_and_yields_tested_seeds() {
        let mut entropy =
            KeygenEntropy::new(Scripted::good(STARTUP_SAMPLES + 64)).expect("start-up");
        let status = entropy.status();
        assert!(status.startup_passed);
        assert_eq!(status.samples_tested, STARTUP_SAMPLES as u64);
        let a = entropy.seed::<32>().expect("seed");
        let b = entropy.seed::<32>().expect("seed");
        assert_ne!(*a, *b);
        assert_eq!(entropy.status().samples_tested, STARTUP_SAMPLES as u64 + 64);
        assert_eq!(entropy.status().failures, 0);
    }

    #[test]
    fn a_source_that_sticks_after_startup_fails_closed_and_stays_failed() {
        let mut stream = Scripted::good(STARTUP_SAMPLES).stream;
        stream.extend_from_slice(&[0u8; 64]);
        let mut entropy = KeygenEntropy::new(Scripted::new(stream, 8_000)).expect("start-up");
        let mut out = [0xAAu8; 32];
        assert_eq!(
            entropy.fill(&mut out),
            Err(KeygenEntropyError::Health(HealthFailure::RepetitionCount {
                cutoff: 4
            }))
        );
        assert_eq!(out, [0u8; 32], "no untested output on failure");
        // Latched: the next request is refused without drawing again.
        assert!(matches!(
            entropy.seed::<32>(),
            Err(KeygenEntropyError::Health(_))
        ));
        let status = entropy.status();
        assert_eq!(status.failures, 2);
        assert_eq!(
            status.last_failure,
            Some(HealthFailure::RepetitionCount { cutoff: 4 })
        );
    }

    #[test]
    fn a_source_error_is_a_typed_error_not_a_fallback() {
        let short = Scripted::new(vec![1, 2, 3], 8_000);
        assert!(matches!(
            KeygenEntropy::new(short),
            Err(KeygenEntropyError::Source(EntropyError::SourceFailed(_)))
        ));
    }

    #[test]
    fn failure_messages_carry_counts_never_sample_values() {
        let shown = KeygenEntropyError::Health(HealthFailure::AdaptiveProportion {
            count: 13,
            cutoff: 13,
        })
        .to_string();
        assert_eq!(
            shown,
            "entropy health test failed: adaptive proportion test: one value 13 times in 512 samples (cutoff 13)"
        );
    }

    #[test]
    fn the_os_source_starts_up() {
        // `new` retries once itself; both attempts fail on a healthy source
        // about 1 in 15.7 million runs (see `STARTUP_ATTEMPTS`).
        let entropy = KeygenEntropy::os().expect("two start-up failures");
        assert!(entropy.status().startup_passed);
    }
}
