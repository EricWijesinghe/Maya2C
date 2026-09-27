//! Frequency-responsive pricing and green-energy certificates.
//!
//! **The grid is SIM.** Nothing here reads a real grid: prices follow the
//! frequency samples they are given, and certificates follow meter readings
//! that must already have been authenticated (in this tree, by a device key
//! under `maya-iot-anchor`). What is tested is the arithmetic: prices move the
//! right way and stay in bounds, and a certificate is minted once per
//! verified megawatt-hour, never twice and never from a meter run backwards.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::EnergyError;

/// Basis points in 1.
pub const BPS: u64 = 10_000;
/// Watt-hours in one certificate (1 `MWh`).
pub const WH_PER_CERTIFICATE: u64 = 1_000_000;

/// A price rule: under-frequency (demand exceeds supply) raises the price,
/// over-frequency lowers it, linearly in Δf and clamped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrequencyPricing {
    /// Nominal frequency, mHz.
    pub nominal_mhz: u32,
    /// Price at nominal, in the smallest currency unit per kWh.
    pub base: u64,
    /// Basis points of price change per mHz of deviation.
    pub bps_per_mhz: u64,
    /// Lowest multiplier, bps.
    pub floor_bps: u64,
    /// Highest multiplier, bps.
    pub cap_bps: u64,
}

impl FrequencyPricing {
    /// The price at `f_mhz`.
    #[must_use]
    pub fn price(&self, f_mhz: u32) -> u64 {
        let deviation = u64::from(self.nominal_mhz.abs_diff(f_mhz));
        let shift = deviation.saturating_mul(self.bps_per_mhz);
        let bps = if f_mhz < self.nominal_mhz {
            BPS.saturating_add(shift)
        } else {
            BPS.saturating_sub(shift)
        }
        .clamp(self.floor_bps, self.cap_bps);
        u64::try_from(u128::from(self.base) * u128::from(bps) / u128::from(BPS)).unwrap_or(u64::MAX)
    }
}

/// A minted certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Certificate {
    /// The meter that generated it.
    pub meter: [u8; 32],
    /// Its serial for that meter, from 0.
    pub serial: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct Meter {
    /// Cumulative reading at enrollment, Wh.
    enrolled_at: u64,
    /// Last accepted cumulative reading, Wh.
    reading: u64,
    /// Certificates minted so far.
    minted: u64,
}

/// Certificates per meter, and which are retired against an offset claim.
#[derive(Clone, Debug, Default)]
pub struct CertificateRegistry {
    meters: BTreeMap<[u8; 32], Meter>,
    retired: BTreeMap<Certificate, [u8; 32]>,
}

impl CertificateRegistry {
    /// Registers a meter at its current cumulative reading, so energy
    /// generated before enrollment earns nothing.
    ///
    /// # Errors
    ///
    /// The meter is already registered.
    pub fn enroll(&mut self, meter: [u8; 32], reading_wh: u64) -> Result<(), EnergyError> {
        if self.meters.contains_key(&meter) {
            return Err(EnergyError::Refused("meter already enrolled".into()));
        }
        self.meters.insert(
            meter,
            Meter {
                enrolled_at: reading_wh,
                reading: reading_wh,
                minted: 0,
            },
        );
        Ok(())
    }

    /// Accepts an authenticated cumulative reading and mints one certificate
    /// per whole `MWh` generated since enrollment that has not yet been
    /// certified. The remainder carries over to the next reading.
    ///
    /// # Errors
    ///
    /// An unenrolled meter, or a reading below the last one (a replaced or
    /// tampered meter: the operator must enroll it again under a new id).
    pub fn record(
        &mut self,
        meter: [u8; 32],
        reading_wh: u64,
    ) -> Result<Vec<Certificate>, EnergyError> {
        let m = self
            .meters
            .get_mut(&meter)
            .ok_or_else(|| EnergyError::Refused("meter not enrolled".into()))?;
        if reading_wh < m.reading {
            return Err(EnergyError::Refused(format!(
                "meter reading went from {} Wh to {reading_wh} Wh",
                m.reading
            )));
        }
        m.reading = reading_wh;
        let earned = (reading_wh - m.enrolled_at) / WH_PER_CERTIFICATE;
        let new = (m.minted..earned)
            .map(|serial| Certificate { meter, serial })
            .collect();
        m.minted = earned;
        Ok(new)
    }

    /// Retires `certificate` against the offset claim `claim`, once.
    ///
    /// # Errors
    ///
    /// A certificate never minted, or one already retired.
    pub fn retire(&mut self, certificate: Certificate, claim: [u8; 32]) -> Result<(), EnergyError> {
        let minted = self
            .meters
            .get(&certificate.meter)
            .is_some_and(|m| certificate.serial < m.minted);
        if !minted {
            return Err(EnergyError::Refused("certificate was never minted".into()));
        }
        if let Some(by) = self.retired.get(&certificate) {
            return Err(EnergyError::Refused(format!(
                "already retired by claim {}",
                hex_short(by)
            )));
        }
        self.retired.insert(certificate, claim);
        Ok(())
    }

    /// Certificates retired against `claim`.
    #[must_use]
    pub fn retired_by(&self, claim: &[u8; 32]) -> usize {
        self.retired.values().filter(|c| *c == claim).count()
    }
}

fn hex_short(bytes: &[u8; 32]) -> String {
    bytes[..4].iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}
