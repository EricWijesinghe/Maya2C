//! SIM GPU: a deterministic model that carries the auto-tuner's tests. It is
//! not hardware, and it says so in its backend name and in its log. Compiled
//! for tests only, so no release build can select it.

use crate::autotune::Sample;
use crate::gpu_control::{ControlError, GpuControl, Knobs};

const SIM_POWER_W: f64 = 250.0;
const SIM_CLOCK_MHZ: f64 = 2700.0;
/// The SIM card's stock settings.
pub const SIM_STOCK: Knobs = Knobs {
    power_w: SIM_POWER_W,
    clock_mhz: Some(SIM_CLOCK_MHZ),
};
const SIM_BACKEND: &str = "SIM (simulated GPU, not hardware)";
/// Draw at full clock, as a share of the stock power limit.
const DRAW_AT_STOCK: f64 = 0.9;
/// Hash rate at full clock, in arbitrary SIM units.
const HASH_AT_STOCK: f64 = 100.0;
/// Hash rate stops rising with clock below this share of the stock clock.
const CLOCK_KNEE: f64 = 0.8;
const TEMP_BASE_C: f64 = 35.0;
const TEMP_SPAN_C: f64 = 40.0;

/// A simulated card. Its log lines all start with `SIM`.
pub struct SimGpu {
    current: Knobs,
    pub log: Vec<String>,
}

impl SimGpu {
    #[must_use]
    pub fn new() -> Self {
        Self {
            current: SIM_STOCK,
            log: Vec::new(),
        }
    }

    /// One reading at the current settings, as a running miner would report it.
    #[must_use]
    pub fn sample(&self) -> Sample {
        let (hps, power_w, temp_c) = model(self.current);
        Sample {
            hps,
            power_w,
            temp_c,
            running: true,
            restarts: 0,
        }
    }
}

impl Default for SimGpu {
    fn default() -> Self {
        Self::new()
    }
}

/// Draw follows clock squared, capped by the limit. Hash rate rises with the
/// clock up to a knee and then holds. Returns `(hps, watts, celsius)`.
fn model(k: Knobs) -> (f64, f64, f64) {
    let f = k.clock_mhz.unwrap_or(SIM_CLOCK_MHZ) / SIM_CLOCK_MHZ;
    let draw = k.power_w.min(DRAW_AT_STOCK * SIM_POWER_W * f * f);
    let hps = HASH_AT_STOCK * (f / CLOCK_KNEE).min(1.0);
    let temp = TEMP_BASE_C + TEMP_SPAN_C * draw / SIM_POWER_W;
    (hps, draw, temp)
}

impl GpuControl for SimGpu {
    fn backend(&self) -> &'static str {
        SIM_BACKEND
    }

    fn stock(&self) -> Result<Knobs, ControlError> {
        Ok(SIM_STOCK)
    }

    fn apply(&mut self, knobs: Knobs) -> Result<(), ControlError> {
        let within_card = knobs.power_w > 0.0
            && knobs.power_w <= SIM_POWER_W
            && knobs
                .clock_mhz
                .is_some_and(|c| c > 0.0 && c <= SIM_CLOCK_MHZ);
        if !within_card {
            self.log.push(format!(
                "SIM refused {knobs:?}: above the SIM card's own limits"
            ));
            return Err(ControlError::Failed(
                "SIM: the card refuses that setting".into(),
            ));
        }
        self.log.push(format!("SIM apply {knobs:?}"));
        self.current = knobs;
        Ok(())
    }

    fn reset(&mut self, stock: Knobs) -> Result<(), ControlError> {
        self.log.push("SIM reset".into());
        self.current = stock;
        Ok(())
    }
}
