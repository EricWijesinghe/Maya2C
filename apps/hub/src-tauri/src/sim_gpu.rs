//! SIM GPU: a deterministic model that carries the guided tuner's tests. It
//! is not hardware, and it says so in its name and its log. Compiled for
//! tests only, so no release build can select it.
//!
//! The test plays the user: it calls [`SimGpu::set_limit`] with the value
//! the Hub suggested, as a person would in their vendor tool, and the SIM
//! card reads the new limit back on its next sample.

use crate::autotune::{Range, Sample};

/// The SIM card's driver range.
pub const SIM_RANGE: Range = Range {
    min_w: 100.0,
    default_w: 250.0,
};
/// Draw as a share of the limit.
const DRAW_FRAC: f64 = 0.9;
/// Hash rate at the default limit, in arbitrary SIM units.
const HASH_AT_DEFAULT: f64 = 100.0;
/// Hash rate follows draw to this power: concave, so efficiency rises as the
/// limit falls and only the hash-rate floor stops the search.
const HASH_EXPONENT: f64 = 0.3;
const TEMP_BASE_C: f64 = 35.0;
const TEMP_SPAN_C: f64 = 40.0;

/// A simulated card. Its log lines all start with `SIM`.
pub struct SimGpu {
    limit_w: f64,
    pub log: Vec<String>,
}

impl SimGpu {
    #[must_use]
    pub fn new() -> Self {
        Self {
            limit_w: SIM_RANGE.default_w,
            log: Vec::new(),
        }
    }

    /// What the user does in their vendor tool. The SIM card clamps to its
    /// range, as a driver does.
    pub fn set_limit(&mut self, watts: f64) {
        self.limit_w = watts.clamp(SIM_RANGE.min_w, SIM_RANGE.default_w);
        self.log
            .push(format!("SIM limit set to {} W", self.limit_w));
    }

    /// Hash rate at a limit.
    #[must_use]
    pub fn rate_at(&self, limit_w: f64) -> f64 {
        let draw = DRAW_FRAC * limit_w;
        let at_default = DRAW_FRAC * SIM_RANGE.default_w;
        HASH_AT_DEFAULT * (draw / at_default).powf(HASH_EXPONENT)
    }

    /// One reading, as the driver and a running miner would give it.
    #[must_use]
    pub fn sample(&self) -> Sample {
        let draw = DRAW_FRAC * self.limit_w;
        Sample {
            hps: Some(self.rate_at(self.limit_w)),
            power_w: Some(draw),
            limit_w: Some(self.limit_w),
            temp_c: Some(TEMP_BASE_C + TEMP_SPAN_C * draw / SIM_RANGE.default_w),
            running: true,
            ..Sample::default()
        }
    }
}

impl Default for SimGpu {
    fn default() -> Self {
        Self::new()
    }
}

#[test]
fn the_sim_card_says_it_is_simulated() {
    let mut sim = SimGpu::new();
    sim.set_limit(10.0);
    assert_eq!(
        sim.sample().limit_w,
        Some(SIM_RANGE.min_w),
        "clamped like a driver"
    );
    assert!(sim.log.iter().all(|l| l.starts_with("SIM")));
}
