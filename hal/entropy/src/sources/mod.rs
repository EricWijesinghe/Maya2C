//! Entropy sources.

pub mod research;
pub mod sim;

use crate::{EntropyError, EntropySource, MAX_MILLIBITS_PER_BYTE, SourceClass};

/// The operating system's generator (`getrandom`): REAL, full entropy.
#[derive(Debug, Default)]
pub struct OsSource;

impl EntropySource for OsSource {
    fn name(&self) -> &'static str {
        "os"
    }

    fn class(&self) -> SourceClass {
        SourceClass::Real
    }

    fn min_entropy_millibits(&self) -> u32 {
        MAX_MILLIBITS_PER_BYTE
    }

    fn fill(&mut self, out: &mut [u8]) -> Result<(), EntropyError> {
        getrandom::fill(out).map_err(|e| EntropyError::SourceFailed(format!("os: {e}")))
    }
}

/// Intel/AMD `RDSEED`: REAL, x86-64 only, detected at construction.
///
/// `RDSEED` returns output of the CPU's conditioned entropy source (unlike
/// `RDRAND`, which is a DRBG seeded from it). It can transiently fail when the
/// source is drained; each word is retried a bounded number of times, and a
/// persistent failure is an error, never a zero.
///
/// Declared at 4 bits per byte, half of what Intel claims: the claim cannot be
/// checked from software, and the pool never relies on this source alone.
#[derive(Debug)]
pub struct RdSeedSource {
    _detected: (),
}

/// Retries per 64-bit word before `RDSEED` is reported as failed. Intel's
/// guidance is to retry with a pause; 1,024 attempts is well past any
/// transient underflow.
const RDSEED_RETRIES: u32 = 1_024;

impl RdSeedSource {
    /// `Some` only on an x86-64 CPU that advertises `RDSEED`.
    #[must_use]
    pub fn detect() -> Option<Self> {
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("rdseed") {
                return Some(Self { _detected: () });
            }
        }
        None
    }
}

#[cfg(target_arch = "x86_64")]
fn rdseed_word() -> Option<u64> {
    // A safe function with a target feature: its body may use the `rdseed`
    // intrinsic directly, and only *calling* it needs `unsafe`, because the
    // caller must promise the CPU has the feature.
    #[target_feature(enable = "rdseed")]
    fn step(out: &mut u64) -> i32 {
        core::arch::x86_64::_rdseed64_step(out)
    }
    let mut word = 0u64;
    for _ in 0..RDSEED_RETRIES {
        // SAFETY: an `RdSeedSource` is only constructed by `detect()` after
        // `is_x86_feature_detected!("rdseed")` returned true on this CPU, and
        // this function is only reached through one.
        if unsafe { step(&mut word) } == 1 {
            return Some(word);
        }
        core::hint::spin_loop();
    }
    None
}

impl EntropySource for RdSeedSource {
    fn name(&self) -> &'static str {
        "rdseed"
    }

    fn class(&self) -> SourceClass {
        SourceClass::Real
    }

    fn min_entropy_millibits(&self) -> u32 {
        4_000
    }

    fn fill(&mut self, out: &mut [u8]) -> Result<(), EntropyError> {
        #[cfg(target_arch = "x86_64")]
        {
            for chunk in out.chunks_mut(8) {
                let word = rdseed_word().ok_or_else(|| {
                    EntropyError::SourceFailed("rdseed: retries exhausted".into())
                })?;
                chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
            }
            Ok(())
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            let _ = out;
            Err(EntropyError::Unavailable(
                "rdseed".into(),
                "not an x86-64 target",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::HealthMonitor;

    #[test]
    fn the_os_source_passes_the_health_tests() {
        let mut source = OsSource;
        let mut buf = vec![0u8; 65_536];
        source.fill(&mut buf).expect("os");
        assert_eq!(
            HealthMonitor::new(source.min_entropy_millibits()).check(&buf),
            Ok(())
        );
    }

    #[test]
    fn rdseed_when_present_produces_healthy_nonconstant_output() {
        let Some(mut source) = RdSeedSource::detect() else {
            eprintln!("rdseed: not present on this CPU; nothing to test");
            return;
        };
        let mut buf = vec![0u8; 65_536];
        source.fill(&mut buf).expect("rdseed");
        assert!(buf.iter().any(|&b| b != buf[0]));
        assert_eq!(
            HealthMonitor::new(source.min_entropy_millibits()).check(&buf),
            Ok(())
        );
    }
}
