//! **SIM** physical entropy sources.
//!
//! Each models a device's *output distribution*: Gaussian physical noise,
//! digitised by an ADC, with the device's deterministic components added. The
//! randomness driving the Gaussian comes from the OS generator, so these add
//! **no entropy** — they exist to give the health tests and the pool
//! realistic, fault-injectable inputs. Every one is named `sim-…`, reports
//! [`SourceClass::Sim`], and logs a warning when constructed.
//!
//! Each declares a min-entropy *derived from its own model*: the probability
//! of the most likely output byte, computed from the Gaussian's width in ADC
//! steps, with a 10 % margin. A wrong model gives a wrong claim, and the
//! health tests are only as strict as the claim — the reason the pool never
//! counts on a single source.
//!
//! Floating point is fine here: nothing in this module is a consensus rule.

// The models convert between physical quantities (f64) and ADC codes (integers)
// on every sample; each cast is the quantisation the model describes, bounded
// by construction (codes are reduced mod 256, bins are clamped to 0..=255).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use crate::{EntropyError, EntropySource, MAX_MILLIBITS_PER_BYTE, SourceClass};

/// Boltzmann's constant, J/K (exact since the 2019 SI).
const BOLTZMANN: f64 = 1.380_649e-23;

/// Standard normal samples from OS randomness, via Box–Muller.
struct OsNormal {
    buf: [u8; 4096],
    pos: usize,
    spare: Option<f64>,
}

impl OsNormal {
    fn new() -> Self {
        Self {
            buf: [0; 4096],
            pos: 4096,
            spare: None,
        }
    }

    fn uniform(&mut self) -> Result<f64, EntropyError> {
        if self.pos + 8 > self.buf.len() {
            getrandom::fill(&mut self.buf)
                .map_err(|e| EntropyError::SourceFailed(format!("sim: {e}")))?;
            self.pos = 0;
        }
        let word = u64::from_le_bytes(
            self.buf[self.pos..self.pos + 8]
                .try_into()
                .expect("8 bytes"),
        );
        self.pos += 8;
        // 53 random bits into (0, 1]: never zero, so `ln` is finite.
        Ok(((word >> 11) as f64 + 1.0) / (1u64 << 53) as f64)
    }

    fn sample(&mut self) -> Result<f64, EntropyError> {
        if let Some(z) = self.spare.take() {
            return Ok(z);
        }
        let (u1, u2) = (self.uniform()?, self.uniform()?);
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = core::f64::consts::TAU * u2;
        self.spare = Some(r * theta.sin());
        Ok(r * theta.cos())
    }
}

/// Φ(x), the standard normal CDF, via erfc (Abramowitz & Stegun 7.1.26,
/// |error| < 1.5·10⁻⁷ — ample for an entropy *estimate*).
fn normal_cdf(x: f64) -> f64 {
    let z = x.abs() / core::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * z);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erfc = poly * (-z * z).exp();
    if x >= 0.0 {
        1.0 - 0.5 * erfc
    } else {
        0.5 * erfc
    }
}

/// Min-entropy, in millibits, of `round(σ·z) mod 256` for z ~ N(0, 1), with
/// the 10 % margin applied.
fn gaussian_lsb_byte_millibits(sigma_steps: f64) -> u32 {
    let reach = (8.0 * sigma_steps).ceil() as i64 + 1;
    let mut mass = [0.0f64; 256];
    for code in -reach..=reach {
        let c = code as f64;
        let p = normal_cdf((c + 0.5) / sigma_steps) - normal_cdf((c - 0.5) / sigma_steps);
        mass[code.rem_euclid(256) as usize] += p;
    }
    let max = mass.iter().copied().fold(0.0, f64::max);
    to_declared_millibits(-max.log2())
}

fn to_declared_millibits(bits: f64) -> u32 {
    ((bits * 0.9 * 1_000.0).floor() as u32).min(MAX_MILLIBITS_PER_BYTE)
}

fn announce(name: &str, detail: &str) {
    tracing::warn!(
        source = name,
        "SIM entropy source constructed: {detail}; a model shaped from OS randomness, adds no entropy"
    );
}

macro_rules! sim_source {
    ($ty:ident, $name:literal) => {
        impl EntropySource for $ty {
            fn name(&self) -> &str {
                $name
            }
            fn class(&self) -> SourceClass {
                SourceClass::Sim
            }
            fn min_entropy_millibits(&self) -> u32 {
                self.millibits
            }
            fn fill(&mut self, out: &mut [u8]) -> Result<(), EntropyError> {
                for byte in out {
                    *byte = self.next_byte()?;
                }
                Ok(())
            }
        }
    };
}

/// Johnson–Nyquist noise across a resistor: `v_rms = √(4·k·T·R·Δf)`,
/// amplified and digitised; the output is the ADC code's low byte.
pub struct ThermalNoise {
    normal: OsNormal,
    sigma_steps: f64,
    millibits: u32,
}

impl ThermalNoise {
    /// Temperature (K), resistance (Ω), bandwidth (Hz), amplifier gain, ADC step (V).
    #[must_use]
    pub fn new(kelvin: f64, ohms: f64, hertz: f64, gain: f64, adc_step_volts: f64) -> Self {
        let v_rms = (4.0 * BOLTZMANN * kelvin * ohms * hertz).sqrt() * gain;
        let sigma_steps = v_rms / adc_step_volts;
        announce(
            "sim-thermal",
            &format!("{kelvin} K, {ohms} Ω, σ = {sigma_steps:.1} ADC steps"),
        );
        Self {
            normal: OsNormal::new(),
            sigma_steps,
            millibits: gaussian_lsb_byte_millibits(sigma_steps),
        }
    }

    /// 300 K, 1 MΩ, 1 MHz, gain 1,000, a 12-bit ADC over 3.3 V.
    #[must_use]
    pub fn typical() -> Self {
        Self::new(300.0, 1.0e6, 1.0e6, 1_000.0, 3.3 / 4096.0)
    }

    fn next_byte(&mut self) -> Result<u8, EntropyError> {
        let code = (self.normal.sample()? * self.sigma_steps).round() as i64;
        Ok(code.rem_euclid(256) as u8)
    }
}
sim_source!(ThermalNoise, "sim-thermal");

/// Supply-rail micro-voltage: a deterministic ripple (which an adversary is
/// assumed to know, so it earns no entropy) plus Gaussian jitter.
pub struct MicroVoltage {
    normal: OsNormal,
    sigma_steps: f64,
    ripple_steps: f64,
    period: u64,
    t: u64,
    millibits: u32,
}

impl MicroVoltage {
    /// Jitter (V rms), ripple amplitude (V), ripple period (samples), ADC step (V).
    #[must_use]
    pub fn new(jitter_volts: f64, ripple_volts: f64, period: u64, adc_step_volts: f64) -> Self {
        let sigma_steps = jitter_volts / adc_step_volts;
        announce(
            "sim-microvoltage",
            &format!("σ = {sigma_steps:.2} steps, ripple {ripple_volts} V"),
        );
        Self {
            normal: OsNormal::new(),
            sigma_steps,
            ripple_steps: ripple_volts / adc_step_volts,
            period: period.max(1),
            t: 0,
            millibits: gaussian_lsb_byte_millibits(sigma_steps),
        }
    }

    /// 2 mV jitter, 10 mV ripple at 1/100 of the sample rate, 0.8 mV steps.
    #[must_use]
    pub fn typical() -> Self {
        Self::new(2.0e-3, 10.0e-3, 100, 3.3 / 4096.0)
    }

    fn next_byte(&mut self) -> Result<u8, EntropyError> {
        let phase = (self.t % self.period) as f64 / self.period as f64;
        self.t += 1;
        let ripple = self.ripple_steps * (core::f64::consts::TAU * phase).sin();
        let code = (ripple + self.normal.sample()? * self.sigma_steps).round() as i64;
        Ok(code.rem_euclid(256) as u8)
    }
}
sim_source!(MicroVoltage, "sim-microvoltage");

/// A bead in a microfluidic channel, tracked by a camera: each frame's
/// displacement is N(0, 2·D·Δt) with the Stokes–Einstein `D = kT / 6πηr`,
/// quantised to the localisation step.
pub struct Brownian {
    normal: OsNormal,
    sigma_steps: f64,
    millibits: u32,
}

impl Brownian {
    /// Temperature (K), viscosity (Pa·s), bead radius (m), frame interval (s), step (m).
    #[must_use]
    pub fn new(kelvin: f64, viscosity: f64, radius: f64, dt: f64, step_m: f64) -> Self {
        let diffusion = BOLTZMANN * kelvin / (6.0 * core::f64::consts::PI * viscosity * radius);
        let sigma_steps = (2.0 * diffusion * dt).sqrt() / step_m;
        announce(
            "sim-brownian",
            &format!("D = {diffusion:.3e} m²/s, σ = {sigma_steps:.2} steps"),
        );
        Self {
            normal: OsNormal::new(),
            sigma_steps,
            millibits: gaussian_lsb_byte_millibits(sigma_steps),
        }
    }

    /// A 0.5 µm bead in water at 300 K, 10 ms frames, 20 nm localisation.
    #[must_use]
    pub fn typical() -> Self {
        Self::new(300.0, 1.0e-3, 0.5e-6, 10.0e-3, 20.0e-9)
    }

    fn next_byte(&mut self) -> Result<u8, EntropyError> {
        let code = (self.normal.sample()? * self.sigma_steps).round() as i64;
        Ok(code.rem_euclid(256) as u8)
    }
}
sim_source!(Brownian, "sim-brownian");

/// Vacuum-fluctuation QRNG by balanced homodyne detection.
///
/// A strong local oscillator beats against the vacuum on a 50:50 splitter;
/// the difference photocurrent samples one quadrature of the vacuum, Gaussian
/// with the shot-noise variance (normalised to 1). Classical electronic noise
/// adds variance `10^(−clearance/10)`. An 8-bit ADC spans ±`range` total
/// standard deviations. The declared min-entropy is conditioned on the
/// electronic noise (an adversary may know it): the largest probability any
/// ADC bin gets from the quantum part alone.
pub struct HomodyneQrng {
    normal: OsNormal,
    electronic_sigma: f64,
    step: f64,
    millibits: u32,
}

impl HomodyneQrng {
    /// Shot-noise clearance over electronic noise (dB), ADC half-range in total σ.
    #[must_use]
    pub fn new(clearance_db: f64, range_sigmas: f64) -> Self {
        let electronic_sigma = (10f64.powf(-clearance_db / 10.0)).sqrt();
        let total_sigma = (1.0 + electronic_sigma * electronic_sigma).sqrt();
        let step = 2.0 * range_sigmas * total_sigma / 256.0;
        let centre_bin = 2.0 * normal_cdf(step / 2.0) - 1.0;
        let edge_bin = 1.0 - normal_cdf(range_sigmas * total_sigma - step);
        let millibits = to_declared_millibits(-centre_bin.max(edge_bin).log2());
        announce(
            "sim-homodyne-qrng",
            &format!("{clearance_db} dB clearance, {millibits} mbit/byte"),
        );
        Self {
            normal: OsNormal::new(),
            electronic_sigma,
            step,
            millibits,
        }
    }

    /// 15 dB clearance, ADC over ±4 σ — typical of published CV-QRNGs.
    #[must_use]
    pub fn typical() -> Self {
        Self::new(15.0, 4.0)
    }

    fn next_byte(&mut self) -> Result<u8, EntropyError> {
        let x = self.normal.sample()? + self.electronic_sigma * self.normal.sample()?;
        let code = (x / self.step).floor() + 128.0;
        Ok(code.clamp(0.0, 255.0) as u8)
    }
}
sim_source!(HomodyneQrng, "sim-homodyne-qrng");

/// A fault to inject into any source, after `after` good bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Every byte becomes `value`.
    StuckAt(u8),
    /// One byte in `every` becomes `value`, the rest pass through.
    Bias {
        /// The favoured value.
        value: u8,
        /// Period of the bias.
        every: u32,
    },
}

/// Wraps a source and breaks it on schedule, to exercise the health tests.
pub struct Faulty<S> {
    inner: S,
    fault: Fault,
    after: u64,
    produced: u64,
    name: String,
}

impl<S: EntropySource> Faulty<S> {
    /// `inner`, failing with `fault` after `after` bytes.
    #[must_use]
    pub fn new(inner: S, fault: Fault, after: u64) -> Self {
        let name = format!("{}+fault", inner.name());
        Self {
            inner,
            fault,
            after,
            produced: 0,
            name,
        }
    }
}

impl<S: EntropySource> EntropySource for Faulty<S> {
    fn name(&self) -> &str {
        &self.name
    }
    fn class(&self) -> SourceClass {
        self.inner.class()
    }
    fn min_entropy_millibits(&self) -> u32 {
        self.inner.min_entropy_millibits()
    }
    fn fill(&mut self, out: &mut [u8]) -> Result<(), EntropyError> {
        self.inner.fill(out)?;
        for byte in out {
            if self.produced >= self.after {
                match self.fault {
                    Fault::StuckAt(v) => *byte = v,
                    Fault::Bias { value, every }
                        if self.produced.is_multiple_of(u64::from(every.max(1))) =>
                    {
                        *byte = value;
                    }
                    Fault::Bias { .. } => {}
                }
            }
            self.produced += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
