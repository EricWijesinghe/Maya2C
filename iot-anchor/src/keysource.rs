//! Where a device's seed comes from.
//!
//! The key is always derived from a [`Seed`]; sources differ only in how the
//! seed survives a power cycle. A secure element or TPM seals it; a PUF
//! regenerates it; tests hold it in memory.

use crate::device::Seed;
use crate::error::IotError;
use crate::puf::{HelperData, RESPONSE_BYTES};

/// Produces the device seed.
pub trait KeySource {
    /// The seed, fresh each call. The caller derives the key and drops it.
    ///
    /// # Errors
    ///
    /// The source's own failure.
    fn seed(&mut self) -> Result<Seed, IotError>;
}

/// A seed held in memory. Tests and simulators only.
///
/// Unlike a PUF or TPM source, this keeps its own copy of the seed for as long
/// as it lives, and every call hands out another: the "drop it after use"
/// contract of [`KeySource::seed`] limits the caller's copy, not this one.
pub struct SoftwareSource(Seed);

impl SoftwareSource {
    /// Wraps a seed.
    #[must_use]
    pub const fn new(seed: Seed) -> Self {
        Self(seed)
    }
}

impl KeySource for SoftwareSource {
    fn seed(&mut self) -> Result<Seed, IotError> {
        Ok(self.0.clone())
    }
}

/// A PUF read through a board-specific closure (on ESP32 or STM32: uninitialised
/// SRAM captured before the runtime zeroes it) and corrected with helper data.
pub struct PufSource<F> {
    helper: HelperData,
    read: F,
}

impl<F: FnMut(&mut [u8; RESPONSE_BYTES])> PufSource<F> {
    /// A source over `helper`, reading responses with `read`.
    pub const fn new(helper: HelperData, read: F) -> Self {
        Self { helper, read }
    }
}

impl<F: FnMut(&mut [u8; RESPONSE_BYTES])> KeySource for PufSource<F> {
    fn seed(&mut self) -> Result<Seed, IotError> {
        let mut response = [0u8; RESPONSE_BYTES];
        (self.read)(&mut response);
        let seed = self.helper.reproduce(&response);
        response.fill(0);
        seed
    }
}
