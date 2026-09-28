//! From a recording to a template: spectral power per channel and frequency,
//! binarised against a population reference, and the stimulus-response
//! check.
//!
//! Floating point is fine here: this runs on a device, and nothing it
//! computes reaches consensus.

use maya_personhood::{TEMPLATE_BYTES, Template};

use crate::BciError;
use crate::edf::Recording;

/// Channels the template reads.
pub const CHANNELS: usize = 64;
/// Integer frequencies per channel, 1 to 32 Hz.
pub const FREQS: usize = 32;
/// Seconds of signal the features need.
pub const SECONDS: usize = 4;
/// Candidate stimulus frequencies. Half-integers, so over whole seconds
/// they are orthogonal to the integer bins the template reads, and a
/// stimulus does not move the template.
pub const STIMULI: [f64; 4] = [8.5, 10.5, 12.5, 14.5];
/// Occipital channels (visual cortex), where a flicker response appears.
pub const OCCIPITAL: [usize; 3] = [61, 62, 63];
/// How much stronger the challenged frequency must be than the others.
pub const RESPONSE_RATIO: f64 = 4.0;

/// Power of `samples` at `hz` (Goertzel over the whole window).
#[must_use]
pub fn power(samples: &[f64], rate_hz: f64, hz: f64) -> f64 {
    let omega = 2.0 * std::f64::consts::PI * hz / rate_hz;
    let coeff = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0, 0.0);
    for x in samples {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - coeff * s1 * s2) / (samples.len() as f64).powi(2)
}

/// The population reference: the power an average person shows at
/// `(channel, hz)`, the 1/f shape EEG spectra follow. A subject's bit is
/// whether it is above this.
#[must_use]
pub fn reference(hz: usize) -> f64 {
    let amplitude = crate::sim::baseline_amplitude(hz);
    amplitude * amplitude / 4.0
}

fn window(recording: &Recording) -> Result<(f64, Vec<&[f64]>), BciError> {
    if recording.signals.len() < CHANNELS {
        return Err(BciError::Unusable("fewer than 64 channels"));
    }
    let rate = recording.signals[0].rate_hz;
    // A rate from an EDF header is a positive sample count per second; the
    // product is rounded and bounded below before it becomes an index.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let need = (rate * SECONDS as f64).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(CHANNELS);
    for s in &recording.signals[..CHANNELS] {
        if (s.rate_hz - rate).abs() > f64::EPSILON || s.samples.len() < need {
            return Err(BciError::Unusable(
                "channels differ in rate or are too short",
            ));
        }
        out.push(&s.samples[..need]);
    }
    Ok((rate, out))
}

/// The 2,048-bit template: one bit per channel and frequency.
///
/// # Errors
///
/// [`BciError::Unusable`] for too few channels or too short a recording.
pub fn template(recording: &Recording) -> Result<Template, BciError> {
    let (rate, channels) = window(recording)?;
    let mut bits = [0u8; TEMPLATE_BYTES];
    for (c, samples) in channels.iter().enumerate() {
        for f in 0..FREQS {
            let hz = f + 1;
            if power(samples, rate, hz as f64) > reference(hz) {
                let i = c * FREQS + f;
                bits[i / 8] |= 1 << (i % 8);
            }
        }
    }
    Ok(Template(bits))
}

/// Whether the occipital channels respond at `hz` more strongly, by
/// [`RESPONSE_RATIO`], than at every other candidate stimulus.
///
/// # Errors
///
/// As [`template`].
pub fn responds_to(recording: &Recording, hz: f64) -> Result<bool, BciError> {
    let (rate, channels) = window(recording)?;
    let at = |f: f64| -> f64 { OCCIPITAL.iter().map(|c| power(channels[*c], rate, f)).sum() };
    let target = at(hz);
    Ok(STIMULI
        .iter()
        .filter(|f| (**f - hz).abs() > f64::EPSILON)
        .all(|f| target > RESPONSE_RATIO * at(*f)))
}
