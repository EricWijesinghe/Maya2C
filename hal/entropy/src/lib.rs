//! Entropy for key generation — ADR-010.
//!
//! **Never trust one source.** Every source is wrapped in the SP 800-90B
//! continuous health tests ([`health`]); a source that fails is disabled for
//! the life of the pool and reported. What survives is concatenated into the
//! seed of one SP 800-90A HMAC-DRBG ([`drbg`]), and the pool refuses to
//! produce output unless the healthy sources together claim at least 256 bits
//! of min-entropy *and* the operating system is one of them ([`pool`]).
//!
//! | Source | Class | Where |
//! |---|---|---|
//! | OS (`getrandom`) | REAL | [`sources::OsSource`] |
//! | Intel/AMD `RDSEED` | REAL | [`sources::RdSeedSource`] (x86-64) |
//! | Johnson–Nyquist thermal noise | **SIM** | [`sources::sim::ThermalNoise`] |
//! | Supply micro-voltage jitter | **SIM** | [`sources::sim::MicroVoltage`] |
//! | Brownian motion in a microfluidic channel | **SIM** | [`sources::sim::Brownian`] |
//! | Vacuum-fluctuation QRNG, balanced homodyne | **SIM** | [`sources::sim::HomodyneQrng`] |
//! | Casimir-cavity fluctuations | **RESEARCH** | [`sources::research::CasimirCavity`] |
//!
//! A SIM source is a physical model whose randomness comes *from the OS
//! source* and is shaped to look like the device's output. It adds no
//! entropy. It exists so the health tests and the pool's failure handling
//! have realistic, fault-injectable inputs. It says so in its name (`sim-`),
//! in its [`SourceClass`], and in a `WARN` log line when constructed, and a
//! pool built without the `sim-sources` feature refuses to admit it.
//!
//! A tamper event ([`tamper::TamperLine::trip`]) zeroizes every registered
//! key store and latches the pool shut.

pub mod drbg;
pub mod health;
pub mod pool;
pub mod sources;
pub mod tamper;

/// Min-entropy is declared in millibits per byte (0–8,000), an integer so no
/// source's claim depends on float formatting.
pub const MAX_MILLIBITS_PER_BYTE: u32 = 8_000;

/// Whether a source is a device, a model, or neither yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceClass {
    /// A real entropy source on this machine.
    Real,
    /// A model. Adds no entropy; never admitted without `sim-sources`.
    Sim,
    /// Not implemented as a model or a device.
    Research,
}

/// Errors from sources, the DRBG and the pool.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EntropyError {
    /// The underlying device or OS call failed.
    #[error("entropy source {0} failed")]
    SourceFailed(String),
    /// The source does not exist in this build or on this machine.
    #[error("entropy source {0} is unavailable: {1}")]
    Unavailable(String, &'static str),
}

/// Something that produces raw, possibly biased, possibly correlated bytes.
pub trait EntropySource: Send {
    /// Stable name. SIM sources start with `sim-`, RESEARCH with `research-`.
    fn name(&self) -> &str;
    /// REAL, SIM or RESEARCH.
    fn class(&self) -> SourceClass;
    /// Claimed min-entropy per output byte, in millibits (≤ 8,000). The
    /// health-test cutoffs are computed from this, so an optimistic claim
    /// makes the tests weaker — claim low.
    fn min_entropy_millibits(&self) -> u32;
    /// Fills `out` with raw samples, one byte per sample.
    ///
    /// # Errors
    ///
    /// [`EntropyError`] if the device or model fails.
    fn fill(&mut self, out: &mut [u8]) -> Result<(), EntropyError>;
}
