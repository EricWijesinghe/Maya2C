//! Identifiers, sensor classes and declared bounds.

use crate::error::IotError;
use crate::wire::{Reader, Writer, tagged_hash};

/// ML-DSA-65 public key length.
pub const PUBLIC_KEY_BYTES: usize = fips204::ml_dsa_65::PK_LEN;

/// ML-DSA-65 signature length.
pub const SIGNATURE_BYTES: usize = fips204::ml_dsa_65::SIG_LEN;

/// Most readings one batch may cover: one a second for a day. Bounds the range a
/// single signature speaks for.
pub const MAX_BATCH_READINGS: u64 = 86_400;

/// A device's identifier: a hash of its public key, so one key is one device.
pub type DeviceId = [u8; 32];

/// The identifier of `public_key`.
#[must_use]
pub fn device_id(public_key: &[u8; PUBLIC_KEY_BYTES]) -> DeviceId {
    tagged_hash(b"maya2c iot device id v1", &[public_key])
}

/// What a device measures, which fixes the unit of every reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorClass {
    /// Cumulative energy delivered, in watt-hours.
    EnergyMeter,
    /// Temperature, in thousandths of a degree Celsius.
    ColdChainTemperature,
}

impl SensorClass {
    /// Wire tag. Consensus; never renumber.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::EnergyMeter => 1,
            Self::ColdChainTemperature => 2,
        }
    }

    /// The class a tag names.
    ///
    /// # Errors
    ///
    /// [`IotError::UnknownTag`].
    pub const fn from_tag(tag: u8) -> Result<Self, IotError> {
        match tag {
            1 => Ok(Self::EnergyMeter),
            2 => Ok(Self::ColdChainTemperature),
            other => Err(IotError::UnknownTag(other)),
        }
    }
}

/// Why firmware reported tampering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TamperCause {
    /// Enclosure switch or mesh opened.
    Enclosure,
    /// Supply voltage outside the brown-out window (fault injection).
    SupplyVoltage,
    /// Clock glitch detected.
    ClockGlitch,
    /// Light inside a sealed enclosure.
    Light,
}

impl TamperCause {
    /// Wire tag. Consensus; never renumber.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Enclosure => 1,
            Self::SupplyVoltage => 2,
            Self::ClockGlitch => 3,
            Self::Light => 4,
        }
    }

    /// The cause a tag names.
    ///
    /// # Errors
    ///
    /// [`IotError::UnknownTag`].
    pub const fn from_tag(tag: u8) -> Result<Self, IotError> {
        match tag {
            1 => Ok(Self::Enclosure),
            2 => Ok(Self::SupplyVoltage),
            3 => Ok(Self::ClockGlitch),
            4 => Ok(Self::Light),
            other => Err(IotError::UnknownTag(other)),
        }
    }
}

/// The range and per-batch change a device declares at enrollment. Readings
/// outside them are recorded and flagged, never refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    /// Lowest plausible reading.
    pub min: i64,
    /// Highest plausible reading.
    pub max: i64,
    /// Largest plausible change of a batch's min or max from the previous batch.
    pub max_step: u64,
}

/// Encoded length of [`Bounds`].
pub const BOUNDS_BYTES: usize = 24;

impl Bounds {
    /// Checked bounds.
    ///
    /// # Errors
    ///
    /// [`IotError::InvalidRange`] if `min > max`.
    pub const fn new(min: i64, max: i64, max_step: u64) -> Result<Self, IotError> {
        if min > max {
            return Err(IotError::InvalidRange);
        }
        Ok(Self { min, max, max_step })
    }

    pub(crate) fn write(&self, writer: &mut Writer<'_>) {
        writer.i64(self.min);
        writer.i64(self.max);
        writer.u64(self.max_step);
    }

    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, IotError> {
        Self::new(reader.i64()?, reader.i64()?, reader.u64()?)
    }
}
