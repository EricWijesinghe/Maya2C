//! SIM and RESEARCH link models. Each states the physics it takes from the
//! literature in one line, and nothing more is claimed.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use crate::{Class, Link, PPM, Profile};

/// Free-space optical link. SIM.
///
/// Weather adds attenuation in dB; above the fade margin (15 dB) the beam is
/// considered lost and the link reports down — the trigger for RF fallback.
/// Beam steering is modelled as a pointing loss that grows with platform
/// jitter.
#[derive(Clone, Copy, Debug)]
pub struct FsoLaser {
    /// Weather attenuation, tenths of a dB.
    pub weather_decidb: u32,
    /// Pointing jitter, microradians.
    pub jitter_urad: u32,
}

/// Fade above which an FSO link is down, tenths of a dB.
pub const FSO_FADE_MARGIN_DECIDB: u32 = 150;

impl Link for FsoLaser {
    fn name(&self) -> &'static str {
        "fso-laser"
    }
    fn class(&self) -> Class {
        Class::Sim
    }
    fn profile(&self) -> Profile {
        // Pointing loss: 1 dB per 10 µrad of jitter beyond a 5 µrad beam.
        let pointing = self.jitter_urad.saturating_sub(5);
        let fade = self.weather_decidb + pointing;
        let down = fade > FSO_FADE_MARGIN_DECIDB;
        Profile {
            mtu: 9_000,
            latency_us: 3_000,
            loss_ppm: if down { PPM } else { fade * 200 },
            bandwidth_bps: if down { 0 } else { 10_000_000_000 },
            cost_per_mb_micro: 1,
        }
    }
}

/// Ku-band satellite RF (GEO). SIM. Rain fade degrades throughput, rarely
/// drops the link.
#[derive(Clone, Copy, Debug)]
pub struct SatelliteRf {
    /// Rain rate, mm/h.
    pub rain_mm_h: u32,
}

impl Link for SatelliteRf {
    fn name(&self) -> &'static str {
        "satellite-ku"
    }
    fn class(&self) -> Class {
        Class::Sim
    }
    fn profile(&self) -> Profile {
        let degrade = (self.rain_mm_h * 2).min(90); // percent of throughput lost
        Profile {
            mtu: 1_500,
            latency_us: 270_000, // GEO: ~35,786 km each way
            loss_ppm: 5_000 + self.rain_mm_h * 500,
            bandwidth_bps: 50_000_000 * u64::from(100 - degrade) / 100,
            cost_per_mb_micro: 5_000,
        }
    }
}

/// Subsea acoustic modem. SIM. Sound in seawater travels ~1,500 m/s, so a
/// 15 km hop is 10 s one way — the Tier 3 delay of Master Prompt 4 §8.
#[derive(Clone, Copy, Debug)]
pub struct SubseaAcoustic {
    /// Hop length, metres.
    pub distance_m: u64,
}

/// Speed of sound in seawater, m/s.
pub const SOUND_SEAWATER_M_S: u64 = 1_500;

impl Link for SubseaAcoustic {
    fn name(&self) -> &'static str {
        "subsea-acoustic"
    }
    fn class(&self) -> Class {
        Class::Sim
    }
    fn profile(&self) -> Profile {
        Profile {
            mtu: 256,
            latency_us: self.distance_m * 1_000_000 / SOUND_SEAWATER_M_S,
            loss_ppm: 100_000,
            bandwidth_bps: 9_600,
            cost_per_mb_micro: 0,
        }
    }
}

/// Orbital-angular-momentum multiplexing over FSO. SIM. Each OAM mode is a
/// channel; turbulence distorts the phase front and crosstalk removes modes,
/// which phase-front recovery (adaptive optics) partly restores.
#[derive(Clone, Copy, Debug)]
pub struct Oam {
    /// Modes transmitted.
    pub modes: u32,
    /// Turbulence strength, 0-100.
    pub turbulence: u32,
    /// Whether adaptive-optics phase-front recovery is on.
    pub recovery: bool,
}

impl Link for Oam {
    fn name(&self) -> &'static str {
        "oam-fso"
    }
    fn class(&self) -> Class {
        Class::Sim
    }
    fn profile(&self) -> Profile {
        let lost = if self.recovery {
            self.turbulence / 25
        } else {
            self.turbulence / 8
        };
        let usable = self.modes.saturating_sub(lost);
        Profile {
            mtu: 9_000,
            latency_us: 3_000,
            loss_ppm: if usable == 0 { PPM } else { 1_000 },
            bandwidth_bps: u64::from(usable) * 10_000_000_000,
            cost_per_mb_micro: 1,
        }
    }
}

/// Neutrino signalling. **RESEARCH SIM.** The one real demonstration (`MINERvA`,
/// 2012) moved about 0.1 bit/s through 240 m of rock with a 1% error rate;
/// this models that rate, and a repetition code for SNR below −20 dB.
#[derive(Clone, Copy, Debug)]
pub struct Neutrino;

impl Link for Neutrino {
    fn name(&self) -> &'static str {
        "neutrino"
    }
    fn class(&self) -> Class {
        Class::Research
    }
    fn profile(&self) -> Profile {
        Profile {
            mtu: 32,
            latency_us: 1_000,
            loss_ppm: 10_000,
            bandwidth_bps: 0,
            cost_per_mb_micro: u64::MAX,
        }
    }
}

/// Bits per decibi-second (0.1 bit/s): the neutrino link's raw rate.
pub const NEUTRINO_BITS_PER_10_S: u64 = 1;

/// Majority-vote repetition decoding: the decoder that lets a link with a
/// per-bit error rate below one half deliver correct bits at any SNR, by
/// spending time. Returns the decoded bit for `votes` noisy copies.
pub fn repetition_decode(votes: &[bool]) -> bool {
    votes.iter().filter(|b| **b).count() * 2 > votes.len()
}

/// A chain of quantum repeaters: Werner-state fidelity after entanglement
/// swapping and purification. SIM. Used only to decide whether a QKD path
/// can supply key at all (the `F > 0.95` gate); it carries no data.
#[derive(Clone, Copy, Debug)]
pub struct RepeaterChain {
    /// Elementary link length, km.
    pub segment_km: u32,
    /// Number of segments.
    pub segments: u32,
    /// Fidelity of one elementary link.
    pub link_fidelity: f64,
    /// Purification rounds (BBPSSW) applied after every swap.
    pub purification_rounds: u32,
}

/// Fidelity below which a QKD path is not used (Master Prompt 7 §4).
pub const FIDELITY_GATE: f64 = 0.95;

fn swap(f1: f64, f2: f64) -> f64 {
    // Werner states: F = F1·F2 + (1 − F1)(1 − F2)/3.
    f1 * f2 + (1.0 - f1) * (1.0 - f2) / 3.0
}

fn purify(f: f64) -> f64 {
    // BBPSSW on two copies of fidelity F (success-conditioned).
    let e = (1.0 - f) / 3.0;
    let num = f * f + e * e;
    let den = f * f + 2.0 * f * e + 5.0 * e * e;
    num / den
}

impl RepeaterChain {
    /// End-to-end fidelity after nested swapping.
    pub fn fidelity(&self) -> f64 {
        let mut f = self.link_fidelity;
        for _ in 0..self.purification_rounds {
            f = purify(f);
        }
        let mut span = 1u32;
        let mut end = f;
        while span < self.segments {
            end = swap(end, end);
            for _ in 0..self.purification_rounds {
                end = purify(end);
            }
            span *= 2;
        }
        end
    }

    /// Chain length, km.
    pub fn length_km(&self) -> u32 {
        self.segment_km * self.segments
    }

    /// Whether the chain passes the fidelity gate.
    pub fn passes_gate(&self) -> bool {
        self.fidelity() > FIDELITY_GATE
    }
}
