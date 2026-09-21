//! The entropy pool: every healthy source into one HMAC-DRBG — ADR-010.
//!
//! Seeding draws from *every* healthy source, runs each one's samples through
//! its health monitor, and concatenates what passed into the DRBG's entropy
//! input. It refuses to seed unless
//!
//! 1. the OS source is among the healthy ones — the one source whose failure
//!    would also break everything else on the machine, so the one whose
//!    presence is a floor rather than a bonus; and
//! 2. the healthy sources' declared contributions sum to ≥ 256 bits.
//!
//! A source that fails a health test is disabled for the life of the pool and
//! its bytes from that draw are discarded. The DRBG is reseeded from a fresh
//! draw every [`RESEED_EVERY`] requests, and a tripped tamper line wipes it
//! and shuts the pool.

use zeroize::Zeroizing;

use crate::drbg::{DrbgError, HmacDrbg, MAX_REQUEST_BYTES};
use crate::health::{HealthFailure, HealthMonitor};
use crate::sources::OsSource;
use crate::tamper::TamperLine;
use crate::{EntropyError, EntropySource, SourceClass};

/// Min-entropy the healthy sources must jointly claim before the DRBG seeds.
pub const REQUIRED_BITS: u64 = 256;

/// Generate calls between reseeds from fresh source output.
pub const RESEED_EVERY: u32 = 1_024;

/// Largest draw from one source per seeding.
const MAX_SOURCE_BYTES: usize = 4_096;

/// The DRBG nonce length, SP 800-90A §8.6.7 (half the security strength).
const NONCE_LEN: usize = 16;

/// Why a source stopped contributing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disabled {
    /// It failed a continuous health test.
    Health(HealthFailure),
    /// The device or model reported an error.
    Source(EntropyError),
}

/// One source's standing, for operators.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReport {
    /// Its name.
    pub name: String,
    /// REAL / SIM / RESEARCH.
    pub class: SourceClass,
    /// Declared min-entropy, millibits per byte.
    pub millibits: u32,
    /// Samples health-tested so far.
    pub samples: u64,
    /// `None` while healthy.
    pub disabled: Option<Disabled>,
}

/// Pool errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PoolError {
    /// A SIM or RESEARCH source, without the `sim-sources` feature (RESEARCH
    /// is refused always).
    #[error("{name} ({class:?}) is not admitted to this pool")]
    NotAdmitted {
        /// The source.
        name: String,
        /// Its class.
        class: SourceClass,
    },
    /// The OS source is disabled.
    #[error("the OS entropy source is unhealthy")]
    OsSourceUnhealthy,
    /// Healthy sources claim less than [`REQUIRED_BITS`].
    #[error("healthy sources claim {got} bits of min-entropy, {REQUIRED_BITS} required")]
    InsufficientEntropy {
        /// What they claim.
        got: u64,
    },
    /// The tamper line tripped.
    #[error("tamper line tripped; the pool is shut")]
    Tampered,
    /// The DRBG refused.
    #[error(transparent)]
    Drbg(#[from] DrbgError),
}

struct Monitored {
    source: Box<dyn EntropySource>,
    monitor: HealthMonitor,
    disabled: Option<Disabled>,
}

/// The pool.
pub struct EntropyPool {
    sources: Vec<Monitored>,
    drbg: Option<HmacDrbg>,
    personalization: Vec<u8>,
    tamper: TamperLine,
    since_reseed: u32,
}

impl core::fmt::Debug for EntropyPool {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EntropyPool")
            .field("sources", &self.report())
            .field("seeded", &self.drbg.is_some())
            .finish_non_exhaustive()
    }
}

impl EntropyPool {
    /// A pool holding the OS source, personalized with `personalization`
    /// (SP 800-90A: e.g. a device serial), watching `tamper`.
    #[must_use]
    pub fn new(personalization: &[u8], tamper: TamperLine) -> Self {
        let os: Box<dyn EntropySource> = Box::new(OsSource);
        Self {
            sources: vec![monitored(os)],
            drbg: None,
            personalization: personalization.to_vec(),
            tamper,
            since_reseed: 0,
        }
    }

    /// Adds a source.
    ///
    /// # Errors
    ///
    /// [`PoolError::NotAdmitted`] for RESEARCH always, and for SIM unless the
    /// crate is built with `sim-sources`.
    pub fn add_source(&mut self, source: Box<dyn EntropySource>) -> Result<(), PoolError> {
        let admitted = match source.class() {
            SourceClass::Real => true,
            SourceClass::Sim => cfg!(feature = "sim-sources"),
            SourceClass::Research => false,
        };
        if !admitted {
            return Err(PoolError::NotAdmitted {
                name: source.name().to_owned(),
                class: source.class(),
            });
        }
        self.sources.push(monitored(source));
        Ok(())
    }

    /// Every source's standing.
    #[must_use]
    pub fn report(&self) -> Vec<SourceReport> {
        self.sources
            .iter()
            .map(|m| SourceReport {
                name: m.source.name().to_owned(),
                class: m.source.class(),
                millibits: m.source.min_entropy_millibits(),
                samples: m.monitor.samples(),
                disabled: m.disabled.clone(),
            })
            .collect()
    }

    /// Fills `out` with DRBG output, seeding or reseeding first as needed.
    ///
    /// # Errors
    ///
    /// Tamper, an unhealthy OS source, too little healthy entropy, or a DRBG
    /// refusal. Never partial output: on error `out` is zeroed.
    pub fn fill(&mut self, out: &mut [u8]) -> Result<(), PoolError> {
        let result = self.fill_inner(out);
        if result.is_err() {
            out.fill(0);
        }
        result
    }

    fn fill_inner(&mut self, out: &mut [u8]) -> Result<(), PoolError> {
        if self.tamper.is_tripped() {
            self.drbg = None; // drops, and `ZeroizeOnDrop` wipes K and V
            return Err(PoolError::Tampered);
        }
        if self.drbg.is_none() || self.since_reseed >= RESEED_EVERY {
            self.reseed()?;
        }
        let drbg = self.drbg.as_mut().expect("seeded above");
        for chunk in out.chunks_mut(MAX_REQUEST_BYTES) {
            drbg.generate(chunk, &[])?;
        }
        self.since_reseed += 1;
        Ok(())
    }

    /// Draws from every healthy source and (re)seeds the DRBG.
    fn reseed(&mut self) -> Result<(), PoolError> {
        let entropy = self.gather()?;
        if let Some(drbg) = self.drbg.as_mut() {
            drbg.reseed(&entropy, &self.personalization)?;
        } else {
            let mut nonce = [0u8; NONCE_LEN];
            OsSource.fill(&mut nonce).map_err(|e| {
                self.sources[0].disabled = Some(Disabled::Source(e));
                PoolError::OsSourceUnhealthy
            })?;
            self.drbg = Some(HmacDrbg::instantiate(
                &entropy,
                &nonce,
                &self.personalization,
            )?);
        }
        self.since_reseed = 0;
        Ok(())
    }

    fn gather(&mut self) -> Result<Zeroizing<Vec<u8>>, PoolError> {
        let mut entropy = Zeroizing::new(Vec::new());
        let mut claimed_millibits: u64 = 0;
        for m in self.sources.iter_mut().filter(|m| m.disabled.is_none()) {
            let millibits = m.source.min_entropy_millibits().max(1);
            let want = usize::try_from((REQUIRED_BITS * 1_000).div_ceil(u64::from(millibits)))
                .unwrap_or(MAX_SOURCE_BYTES)
                .clamp(32, MAX_SOURCE_BYTES);
            let mut draw = Zeroizing::new(vec![0u8; want]);
            let outcome = m
                .source
                .fill(&mut draw)
                .map_err(Disabled::Source)
                .and_then(|()| m.monitor.check(&draw).map_err(Disabled::Health));
            match outcome {
                Ok(()) => {
                    entropy.extend_from_slice(&draw);
                    claimed_millibits += want as u64 * u64::from(millibits);
                }
                Err(reason) => {
                    tracing::error!(source = m.source.name(), ?reason, "entropy source disabled");
                    m.disabled = Some(reason);
                }
            }
        }
        if self.sources[0].disabled.is_some() {
            return Err(PoolError::OsSourceUnhealthy);
        }
        let got = claimed_millibits / 1_000;
        if got < REQUIRED_BITS {
            return Err(PoolError::InsufficientEntropy { got });
        }
        Ok(entropy)
    }
}

fn monitored(source: Box<dyn EntropySource>) -> Monitored {
    let monitor = HealthMonitor::new(source.min_entropy_millibits());
    Monitored {
        source,
        monitor,
        disabled: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::sim::{Fault, Faulty, ThermalNoise};

    /// A pool whose primary source is `primary` instead of the OS — only for
    /// testing the refusal paths, which a real OS source cannot be made to take.
    fn with_primary(primary: Box<dyn EntropySource>, tamper: TamperLine) -> EntropyPool {
        EntropyPool {
            sources: vec![monitored(primary)],
            drbg: None,
            personalization: b"test".to_vec(),
            tamper,
            since_reseed: 0,
        }
    }

    #[test]
    fn the_os_source_alone_seeds_the_pool() {
        let mut pool = EntropyPool::new(b"device-1", TamperLine::new());
        let (mut a, mut b) = ([0u8; 64], [0u8; 64]);
        pool.fill(&mut a).expect("fill");
        pool.fill(&mut b).expect("fill");
        assert_ne!(a, b);
        assert_eq!(pool.report()[0].name, "os");
    }

    #[test]
    fn a_large_request_spans_several_drbg_calls() {
        let mut pool = EntropyPool::new(b"", TamperLine::new());
        let mut big = vec![0u8; MAX_REQUEST_BYTES * 3 + 7];
        pool.fill(&mut big).expect("fill");
        assert!(big[MAX_REQUEST_BYTES * 3..].iter().any(|&x| x != 0));
    }

    #[test]
    fn the_pool_reseeds_from_the_sources_on_schedule() {
        let mut pool = EntropyPool::new(b"", TamperLine::new());
        let mut buf = [0u8; 8];
        pool.fill(&mut buf).expect("fill");
        let after_seed = pool.report()[0].samples;
        for _ in 0..RESEED_EVERY {
            pool.fill(&mut buf).expect("fill");
        }
        assert!(
            pool.report()[0].samples > after_seed,
            "a reseed drew fresh samples"
        );
    }

    #[test]
    fn research_sources_are_never_admitted() {
        let mut pool = EntropyPool::new(b"", TamperLine::new());
        let refused = pool.add_source(Box::new(crate::sources::research::CasimirCavity));
        assert!(matches!(
            refused,
            Err(PoolError::NotAdmitted {
                class: SourceClass::Research,
                ..
            })
        ));
    }

    #[test]
    fn sim_sources_are_admitted_only_with_the_feature() {
        let mut pool = EntropyPool::new(b"", TamperLine::new());
        let result = pool.add_source(Box::new(ThermalNoise::typical()));
        assert_eq!(result.is_ok(), cfg!(feature = "sim-sources"));
    }

    #[test]
    fn an_unhealthy_primary_source_shuts_the_pool() {
        let stuck = Faulty::new(ThermalNoise::typical(), Fault::StuckAt(0), 0);
        let mut pool = with_primary(Box::new(stuck), TamperLine::new());
        let mut out = [0xFFu8; 32];
        assert_eq!(pool.fill(&mut out), Err(PoolError::OsSourceUnhealthy));
        assert_eq!(out, [0u8; 32], "no partial output on failure");
        assert!(matches!(
            pool.report()[0].disabled,
            Some(Disabled::Health(_))
        ));
    }

    #[test]
    fn a_failing_secondary_source_is_disabled_and_the_pool_carries_on() {
        let mut pool = EntropyPool::new(b"", TamperLine::new());
        let biased = Faulty::new(ThermalNoise::typical(), Fault::StuckAt(7), 0);
        pool.sources.push(monitored(Box::new(biased))); // bypass admission for the test
        let mut out = [0u8; 32];
        pool.fill(&mut out)
            .expect("the OS source still carries the pool");
        let report = pool.report();
        assert!(report[0].disabled.is_none());
        assert!(matches!(
            report[1].disabled,
            Some(Disabled::Health(HealthFailure::RepetitionCount { .. }))
        ));
    }

    #[test]
    fn a_tamper_trip_shuts_the_pool_and_wipes_its_state() {
        let line = TamperLine::new();
        let mut pool = EntropyPool::new(b"", line.clone());
        let mut out = [0u8; 16];
        pool.fill(&mut out).expect("fill before tamper");
        line.trip();
        let mut after = [0xEEu8; 16];
        assert_eq!(pool.fill(&mut after), Err(PoolError::Tampered));
        assert_eq!(after, [0u8; 16]);
        assert!(pool.drbg.is_none(), "DRBG state dropped (and zeroized)");
    }
}
