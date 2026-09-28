//! **SIM.** A synthetic EEG generator. It is not a model of a brain.
//!
//! A subject is a fixed pattern of spectral amplitudes over 64 channels and
//! 1 to 32 Hz, drawn once from its id. It is scattered around a population
//! baseline that falls as 1/f, the shape real EEG spectra have. A session
//! adds random phases, amplitude jitter and white noise, and optionally a
//! flicker response at the stimulus frequency on the occipital channels.
//! That is enough to exercise the pipeline (stable bits for one subject,
//! different bits across subjects, a stimulus that shows) and nothing more:
//! how stable and distinctive human EEG features really are is not known
//! here.

use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};

use crate::edf;
use crate::features::{CHANNELS, FREQS, OCCIPITAL, SECONDS};

/// Sample rate of generated recordings.
pub const RATE_HZ: usize = 256;
/// Spread of a subject's pattern around the baseline, in log amplitude.
const SUBJECT_SPREAD: f64 = 1.0;
/// Session-to-session jitter, in log amplitude.
const SESSION_JITTER: f64 = 0.1;
/// White noise, µV RMS.
const NOISE_UV: f64 = 0.5;
/// Flicker response amplitude, µV.
const STIMULUS_UV: f64 = 8.0;
/// Physical range written to EDF.
const RANGE_UV: f64 = 400.0;

/// The population's amplitude at `hz` (µV): 1/f, 20 µV at 1 Hz.
#[must_use]
pub fn baseline_amplitude(hz: usize) -> f64 {
    20.0 / hz as f64
}

fn uniform(rng: &mut ChaCha20Rng) -> f64 {
    (f64::from(rng.next_u32()) + 0.5) / 4_294_967_296.0
}

/// A standard normal draw (Box–Muller).
fn normal(rng: &mut ChaCha20Rng) -> f64 {
    (-2.0 * uniform(rng).ln()).sqrt() * (2.0 * std::f64::consts::PI * uniform(rng)).cos()
}

/// A synthetic subject.
pub struct Subject {
    /// Amplitude per channel and frequency, µV.
    amplitude: Vec<[f64; FREQS]>,
}

impl Subject {
    /// Subject `id`'s fixed pattern.
    #[must_use]
    pub fn new(id: u64) -> Self {
        let mut rng = ChaCha20Rng::seed_from_u64(id);
        let amplitude = (0..CHANNELS)
            .map(|_| {
                core::array::from_fn(|f| {
                    baseline_amplitude(f + 1) * (SUBJECT_SPREAD * normal(&mut rng)).exp()
                })
            })
            .collect();
        Self { amplitude }
    }

    /// One session's recording as EDF bytes, labelled SIM in its header.
    #[must_use]
    pub fn record(&self, session: u64, stimulus_hz: Option<f64>) -> Vec<u8> {
        let mut rng = ChaCha20Rng::seed_from_u64(session ^ 0x5EED_0F5E_5510_0000);
        let n = RATE_HZ * SECONDS;
        let signals: Vec<Vec<f64>> = self
            .amplitude
            .iter()
            .enumerate()
            .map(|(c, amps)| {
                let parts: Vec<(f64, f64, f64)> = amps
                    .iter()
                    .enumerate()
                    .map(|(f, a)| {
                        (
                            (f + 1) as f64,
                            a * (SESSION_JITTER * normal(&mut rng)).exp(),
                            2.0 * std::f64::consts::PI * uniform(&mut rng),
                        )
                    })
                    .collect();
                let flicker = stimulus_hz.filter(|_| OCCIPITAL.contains(&c));
                (0..n)
                    .map(|t| {
                        let time = t as f64 / RATE_HZ as f64;
                        let tone: f64 = parts
                            .iter()
                            .map(|(hz, a, phase)| {
                                a * (2.0 * std::f64::consts::PI * hz * time + phase).sin()
                            })
                            .sum();
                        let evoked = flicker.map_or(0.0, |hz| {
                            STIMULUS_UV * (2.0 * std::f64::consts::PI * hz * time).sin()
                        });
                        tone + evoked + NOISE_UV * normal(&mut rng)
                    })
                    .collect()
            })
            .collect();
        let labels: Vec<String> = (0..CHANNELS).map(|c| format!("E{c}")).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        edf::write(&refs, RATE_HZ, &signals, RANGE_UV)
    }
}
