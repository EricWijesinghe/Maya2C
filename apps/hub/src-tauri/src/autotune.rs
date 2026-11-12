//! Hub GPU auto-tuner, the decision core (ADR-137, HW-1).
//!
//! Pure: no I/O, no clock, no randomness. The driver in `tuning.rs` feeds it
//! one [`Sample`] per tick and carries out the [`Step`] it returns, so the same
//! samples always give the same decisions. Every setting it asks for stays
//! inside [`Bounds`], which is built from the stock settings and never widens.

use serde::Serialize;

use crate::gpu_control::Knobs;

// Policy limits, not measurements. Changing one needs a recorded sweep in
// docs/mining/gpu-autotune.md.
/// Power never goes below this share of the stock power limit.
pub const POWER_FLOOR_FRAC: f64 = 0.6;
/// The core clock never goes below this share of the stock clock.
pub const CLOCK_FLOOR_FRAC: f64 = 0.7;
/// One search notch on the power limit, as a share of stock.
pub const POWER_STEP_FRAC: f64 = 0.05;
/// One search notch on the core clock, as a share of stock.
pub const CLOCK_STEP_FRAC: f64 = 0.05;
/// Most measured trials in one session.
pub const MAX_TRIALS: u32 = 24;
/// A candidate must beat the best efficiency by this share to be kept, so
/// sampling noise cannot walk the setting around.
pub const MIN_GAIN: f64 = 0.01;
/// Hash rate below this share of stock is a collapse.
pub const COLLAPSE_FRAC: f64 = 0.5;
/// Consecutive collapsed samples before a tuned GPU is rolled back.
pub const COLLAPSE_STREAK: u32 = 2;
/// The tuner never allows a temperature above this, whatever the user asks
/// for. Matches the watchdog's own default (watchdog.rs).
pub const TEMP_CEILING_C: f64 = 80.0;
/// Two candidates in a row that do not win end the search.
const SEARCH_REJECTS: u32 = 2;
/// Samples dropped after each write, so the device can settle.
const SETTLE_SAMPLES: u32 = 1;
/// Samples averaged into one measurement.
const WINDOW_SAMPLES: usize = 3;

/// One reading of a GPU and its miner, taken once per supervisor tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub hps: f64,
    pub power_w: f64,
    pub temp_c: f64,
    pub running: bool,
    pub restarts: u32,
}

/// The limits every write must satisfy: power and clock only ever fall
/// from stock, and never below the floors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub power_min_w: f64,
    pub power_max_w: f64,
    pub clock_min_mhz: Option<f64>,
    pub clock_max_mhz: Option<f64>,
}

impl Bounds {
    /// Bounds around the stock settings.
    #[must_use]
    pub fn around(stock: Knobs) -> Self {
        Self {
            power_min_w: (stock.power_w * POWER_FLOOR_FRAC).ceil(),
            power_max_w: stock.power_w,
            clock_min_mhz: stock.clock_mhz.map(|c| (c * CLOCK_FLOOR_FRAC).ceil()),
            clock_max_mhz: stock.clock_mhz,
        }
    }

    /// True when `k` may be written to the device.
    #[must_use]
    pub fn contains(&self, k: Knobs) -> bool {
        let power_ok = (self.power_min_w..=self.power_max_w).contains(&k.power_w);
        let clock_ok = match (k.clock_mhz, self.clock_min_mhz, self.clock_max_mhz) {
            (None, None, None) => true,
            (Some(c), Some(lo), Some(hi)) => (lo..=hi).contains(&c),
            _ => false,
        };
        power_ok && clock_ok
    }
}

/// Which knob the search moves next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dial {
    Clock,
    Power,
}

impl Dial {
    const fn other(self) -> Self {
        match self {
            Self::Clock => Self::Power,
            Self::Power => Self::Clock,
        }
    }
}

/// Why a session ended in a rollback to stock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    MinerCrashed,
    MinerStopped,
    HashrateCollapse,
    OverTemperature,
    ControlError,
    UserTurnedOff,
    AppExit,
}

impl Reason {
    /// The words the UI shows after "rolled back: ".
    #[must_use]
    pub const fn words(self) -> &'static str {
        match self {
            Self::MinerCrashed => "the miner crashed",
            Self::MinerStopped => "the miner stopped",
            Self::HashrateCollapse => "hash rate collapsed",
            Self::OverTemperature => "GPU over its temperature ceiling",
            Self::ControlError => "the driver refused a setting",
            Self::UserTurnedOff => "tuning turned off",
            Self::AppExit => "the Hub is closing",
        }
    }
}

/// Where the tuner is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Tuning is on, but the miner has not hashed yet.
    Waiting,
    /// Measuring the stock settings, the reference for every trial.
    Baseline,
    /// Measuring one candidate.
    Trial(Knobs),
    /// Settled on the best setting, still watched for collapse and heat.
    Tuned,
    RolledBack(Reason),
}

/// What the driver must do after a sample. `Settle` means the search is
/// over and `Knobs` is the setting to keep.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Hold,
    Apply(Knobs),
    Settle(Knobs),
    Rollback(Reason),
}

/// Efficiency in hash rate per watt drawn.
fn efficiency(s: Sample) -> f64 {
    s.hps / s.power_w
}

/// The mean of a non-empty window, with the flags taken as live.
fn mean(window: &[Sample]) -> Sample {
    let n = window.len() as f64;
    let (hps, power_w, temp_c) = window.iter().fold((0.0, 0.0, 0.0), |acc, s| {
        (acc.0 + s.hps, acc.1 + s.power_w, acc.2 + s.temp_c)
    });
    Sample {
        hps: hps / n,
        power_w: power_w / n,
        temp_c: temp_c / n,
        running: true,
        restarts: 0,
    }
}

/// The setting one notch down on `dial`, or `None` at its bound or when the
/// device has no such knob.
fn notch(best: Knobs, dial: Dial, stock: Knobs, bounds: &Bounds) -> Option<Knobs> {
    let next = match dial {
        Dial::Power => Knobs {
            power_w: (best.power_w - stock.power_w * POWER_STEP_FRAC)
                .round()
                .max(bounds.power_min_w),
            ..best
        },
        Dial::Clock => {
            let current = best.clock_mhz?;
            let floor = bounds.clock_min_mhz?;
            let step = stock.clock_mhz? * CLOCK_STEP_FRAC;
            Knobs {
                clock_mhz: Some((current - step).round().max(floor)),
                ..best
            }
        }
    };
    (next != best && bounds.contains(next)).then_some(next)
}

/// One GPU's search. Starts at stock, tries the saved seed first if there is
/// one, then steps clock and power down while efficiency keeps rising.
pub struct Tuner {
    stock: Knobs,
    bounds: Bounds,
    ceiling_c: f64,
    seed: Option<Knobs>,
    phase: Phase,
    best: Knobs,
    stock_hps: f64,
    stock_eff: f64,
    best_eff: f64,
    dial: Dial,
    trials: u32,
    rejects: u32,
    low_streak: u32,
    settle_left: u32,
    window: Vec<Sample>,
    last_restarts: u32,
}

impl Tuner {
    /// `ceiling_c` is the user's ceiling and is capped at [`TEMP_CEILING_C`].
    /// `seed` is a best setting saved by an earlier session. It is tried first,
    /// and kept only if it beats stock like any other candidate.
    #[must_use]
    pub fn new(stock: Knobs, ceiling_c: f64, seed: Option<Knobs>) -> Self {
        Self {
            stock,
            bounds: Bounds::around(stock),
            ceiling_c: ceiling_c.min(TEMP_CEILING_C),
            seed,
            phase: Phase::Waiting,
            best: stock,
            stock_hps: 0.0,
            stock_eff: 0.0,
            best_eff: 0.0,
            dial: Dial::Clock,
            trials: 0,
            rejects: 0,
            low_streak: 0,
            settle_left: 0,
            window: Vec::new(),
            last_restarts: 0,
        }
    }

    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    #[must_use]
    pub const fn trials(&self) -> u32 {
        self.trials
    }

    /// Efficiency gain over stock, in percent, once a trial has been measured.
    #[must_use]
    pub fn gain_pct(&self) -> Option<f64> {
        let measured = matches!(self.phase, Phase::Trial(_) | Phase::Tuned);
        (measured && self.stock_eff > 0.0).then(|| (self.best_eff / self.stock_eff - 1.0) * 100.0)
    }

    /// Ends the session with a rollback. The driver resets the device to stock.
    pub fn stop(&mut self, reason: Reason) -> Step {
        match self.phase {
            Phase::RolledBack(_) => Step::Hold,
            _ => self.rollback(reason),
        }
    }

    /// Consumes one sample and says what to do about it.
    pub fn on_sample(&mut self, s: Sample) -> Step {
        match self.phase {
            Phase::RolledBack(_) => Step::Hold,
            Phase::Waiting => {
                if s.running {
                    self.phase = Phase::Baseline;
                    self.settle_left = SETTLE_SAMPLES;
                    self.last_restarts = s.restarts;
                }
                Step::Hold
            }
            _ => self.watch(s),
        }
    }

    /// Safety checks run on every sample; the search only on settled ones.
    fn watch(&mut self, s: Sample) -> Step {
        if s.restarts > self.last_restarts {
            return self.rollback(Reason::MinerCrashed);
        }
        self.last_restarts = s.restarts;
        if !s.running {
            return self.rollback(Reason::MinerStopped);
        }
        if s.temp_c > self.ceiling_c {
            return self.rollback(Reason::OverTemperature);
        }
        if self.settle_left > 0 {
            self.settle_left -= 1;
            return Step::Hold;
        }
        if let Some(step) = self.collapse_guard(s) {
            return step;
        }
        match self.phase {
            Phase::Tuned => Step::Hold,
            _ => self.measure(s),
        }
    }

    /// Two low samples in a row roll back to stock. Active only once the
    /// stock hash rate is known.
    fn collapse_guard(&mut self, s: Sample) -> Option<Step> {
        if self.stock_hps <= 0.0 {
            return None;
        }
        self.low_streak = if s.hps < COLLAPSE_FRAC * self.stock_hps {
            self.low_streak + 1
        } else {
            0
        };
        (self.low_streak >= COLLAPSE_STREAK).then(|| self.rollback(Reason::HashrateCollapse))
    }

    /// Adds a sample to the window; a full window is one measurement.
    fn measure(&mut self, s: Sample) -> Step {
        if !(s.hps.is_finite() && s.hps > 0.0 && s.power_w.is_finite() && s.power_w > 0.0) {
            return Step::Hold;
        }
        self.window.push(s);
        if self.window.len() < WINDOW_SAMPLES {
            return Step::Hold;
        }
        let measured = mean(&self.window);
        self.window.clear();
        match self.phase {
            Phase::Trial(k) => self.judge(k, measured),
            _ => self.finish_baseline(measured),
        }
    }

    fn finish_baseline(&mut self, measured: Sample) -> Step {
        self.stock_hps = measured.hps;
        self.stock_eff = efficiency(measured);
        self.best_eff = self.stock_eff;
        self.advance()
    }

    /// Keeps the candidate only if it is inside the power ceiling and beats the
    /// best efficiency by [`MIN_GAIN`]. Otherwise the search moves on.
    fn judge(&mut self, k: Knobs, measured: Sample) -> Step {
        let eff = efficiency(measured);
        let within_power = measured.power_w <= self.bounds.power_max_w;
        if within_power && eff > self.best_eff * (1.0 + MIN_GAIN) {
            self.best = k;
            self.best_eff = eff;
            self.rejects = 0;
        } else {
            self.rejects += 1;
            self.dial = self.dial.other();
        }
        self.advance()
    }

    /// Starts the next trial, or settles when the search is spent.
    fn advance(&mut self) -> Step {
        if let Some(seed) = self.seed.take() {
            if self.bounds.contains(seed) && seed != self.best {
                return self.start_trial(seed);
            }
        }
        loop {
            if self.trials >= MAX_TRIALS || self.rejects >= SEARCH_REJECTS {
                return self.settle();
            }
            match notch(self.best, self.dial, self.stock, &self.bounds) {
                Some(next) => return self.start_trial(next),
                None => {
                    self.rejects += 1;
                    self.dial = self.dial.other();
                }
            }
        }
    }

    fn start_trial(&mut self, k: Knobs) -> Step {
        self.trials += 1;
        self.phase = Phase::Trial(k);
        self.settle_left = SETTLE_SAMPLES;
        Step::Apply(k)
    }

    fn settle(&mut self) -> Step {
        self.phase = Phase::Tuned;
        self.settle_left = SETTLE_SAMPLES;
        self.low_streak = 0;
        Step::Settle(self.best)
    }

    fn rollback(&mut self, reason: Reason) -> Step {
        self.phase = Phase::RolledBack(reason);
        self.window.clear();
        self.low_streak = 0;
        Step::Rollback(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_control::GpuControl;
    use crate::sim_gpu::{SIM_STOCK, SimGpu};

    /// A reading from a running miner, with the flags it always has.
    fn live(hps: f64, power_w: f64, temp_c: f64) -> Sample {
        Sample {
            hps,
            power_w,
            temp_c,
            running: true,
            restarts: 0,
        }
    }

    /// Stock hash rate, draw and temperature of the SIM card.
    fn stock_reading() -> Sample {
        live(100.0, 225.0, 71.0)
    }

    /// Feeds stock readings until the first trial is requested.
    fn into_trial(t: &mut Tuner) -> Step {
        let mut step = Step::Hold;
        for _ in 0..5 {
            step = t.on_sample(stock_reading());
        }
        step
    }

    /// Drives the tuner against the SIM card until it settles, returning the
    /// tuner, the card, and every setting it was asked to write.
    fn run_sim(seed: Option<Knobs>) -> (Tuner, SimGpu, Vec<Knobs>) {
        const RUN_CAP: usize = 400;
        let mut sim = SimGpu::new();
        let mut tuner = Tuner::new(SIM_STOCK, TEMP_CEILING_C, seed);
        let mut applied = Vec::new();
        let mut step = tuner.on_sample(sim.sample());
        for _ in 0..RUN_CAP {
            match step {
                Step::Apply(k) | Step::Settle(k) => {
                    sim.apply(k)
                        .expect("in-bound knobs are inside the SIM card's limits");
                    applied.push(k);
                    if matches!(step, Step::Settle(_)) {
                        break;
                    }
                }
                Step::Rollback(r) => panic!("unexpected rollback: {r:?}"),
                Step::Hold => {}
            }
            step = tuner.on_sample(sim.sample());
        }
        (tuner, sim, applied)
    }

    #[test]
    fn bounds_only_fall_from_stock_and_stop_at_the_floors() {
        let bounds = Bounds::around(SIM_STOCK);
        assert!(bounds.contains(SIM_STOCK));
        assert!(!bounds.contains(Knobs {
            power_w: SIM_STOCK.power_w + 1.0,
            ..SIM_STOCK
        }));
        assert!(!bounds.contains(Knobs {
            clock_mhz: Some(2701.0),
            ..SIM_STOCK
        }));
        assert!(!bounds.contains(Knobs {
            power_w: 149.0,
            ..SIM_STOCK
        }));
        assert!(!bounds.contains(Knobs {
            clock_mhz: Some(1889.0),
            ..SIM_STOCK
        }));
        assert!(bounds.contains(Knobs {
            power_w: 150.0,
            clock_mhz: Some(1890.0)
        }));
    }

    #[test]
    fn notch_walks_each_knob_to_its_floor_and_stops_there() {
        let bounds = Bounds::around(SIM_STOCK);
        for (dial, floor) in [(Dial::Power, 150.0), (Dial::Clock, 1890.0)] {
            let mut k = SIM_STOCK;
            let mut notches = 0;
            while let Some(next) = notch(k, dial, SIM_STOCK, &bounds) {
                assert!(bounds.contains(next), "{next:?} left the bounds");
                k = next;
                notches += 1;
                assert!(notches < 100, "the walk must end at the floor");
            }
            let at_floor = match dial {
                Dial::Power => k.power_w,
                Dial::Clock => k.clock_mhz.expect("SIM card has a clock"),
            };
            assert_eq!(at_floor, floor);
        }
    }

    #[test]
    fn waits_for_a_running_miner_before_measuring() {
        let mut t = Tuner::new(SIM_STOCK, TEMP_CEILING_C, None);
        let idle = Sample {
            running: false,
            ..stock_reading()
        };
        assert_eq!(t.on_sample(idle), Step::Hold);
        assert_eq!(t.phase(), Phase::Waiting);
    }

    #[test]
    fn the_sim_search_is_deterministic_and_keeps_a_gain() {
        let (tuner, sim, applied) = run_sim(None);
        let (_, _, again) = run_sim(None);
        assert_eq!(applied, again, "the same curve gives the same trials");
        assert_eq!(tuner.phase(), Phase::Tuned);
        assert!(tuner.trials() <= MAX_TRIALS);
        let best = *applied.last().expect("the search settled");
        // Hand-derived for the SIM curve: draw falls with clock squared while
        // hash rate holds to the knee, so the clock runs to its floor. Draw at
        // that clock (about 110 W) sits under the power limit, so the power
        // notch never helps and the search keeps the stock limit.
        assert_eq!(best.clock_mhz, Some(1890.0));
        assert_eq!(best.power_w, SIM_STOCK.power_w);
        assert!(tuner.gain_pct().is_some_and(|g| g > 50.0));
        assert!(sim.log.iter().all(|line| line.starts_with("SIM")));
    }

    #[test]
    fn every_setting_asked_for_stays_inside_the_bounds_under_noise() {
        let bounds = Bounds::around(SIM_STOCK);
        let mut lcg: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            lcg = lcg
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (lcg >> 11) as f64 / (1_u64 << 53) as f64
        };
        let mut t = Tuner::new(SIM_STOCK, TEMP_CEILING_C, None);
        let mut asked = 0;
        for _ in 0..5000 {
            let s = Sample {
                hps: 50.0 + 100.0 * next(),
                power_w: 100.0 + 200.0 * next(),
                temp_c: 60.0 + 25.0 * next(),
                running: next() > 0.02,
                restarts: 0,
            };
            match t.on_sample(s) {
                Step::Apply(k) | Step::Settle(k) => {
                    assert!(bounds.contains(k), "{k:?} is outside {bounds:?}");
                    asked += 1;
                }
                Step::Rollback(_) => t = Tuner::new(SIM_STOCK, TEMP_CEILING_C, None),
                Step::Hold => {}
            }
        }
        assert!(asked > 0, "the noise reached at least one trial");
    }

    #[test]
    fn a_saved_seed_is_tried_first_and_an_out_of_bounds_seed_is_ignored() {
        let seed = Knobs {
            power_w: 150.0,
            clock_mhz: Some(1890.0),
        };
        let (_, _, applied) = run_sim(Some(seed));
        assert_eq!(applied.first(), Some(&seed));
        assert_eq!(applied.last(), Some(&seed));

        let outside = Knobs {
            power_w: 100.0,
            clock_mhz: Some(1890.0),
        };
        let (_, _, applied) = run_sim(Some(outside));
        assert_ne!(applied.first(), Some(&outside));
        assert!(applied.iter().all(|k| k.power_w >= 150.0));
    }

    #[test]
    fn a_miner_crash_rolls_back_to_stock_and_then_holds() {
        let mut t = Tuner::new(SIM_STOCK, TEMP_CEILING_C, None);
        assert!(matches!(into_trial(&mut t), Step::Apply(_)));
        let crashed = Sample {
            restarts: 1,
            ..stock_reading()
        };
        assert_eq!(t.on_sample(crashed), Step::Rollback(Reason::MinerCrashed));
        assert_eq!(t.phase(), Phase::RolledBack(Reason::MinerCrashed));
        assert_eq!(
            t.on_sample(stock_reading()),
            Step::Hold,
            "no write after a rollback"
        );
    }

    #[test]
    fn a_stopped_miner_rolls_back() {
        let mut t = Tuner::new(SIM_STOCK, TEMP_CEILING_C, None);
        into_trial(&mut t);
        let stopped = Sample {
            running: false,
            ..stock_reading()
        };
        assert_eq!(t.on_sample(stopped), Step::Rollback(Reason::MinerStopped));
    }

    #[test]
    fn a_temperature_over_the_ceiling_rolls_back_and_the_ceiling_is_capped() {
        // Stock runs at 71 C, so the user's ceiling sits just above it.
        let mut t = Tuner::new(SIM_STOCK, 72.0, None);
        into_trial(&mut t);
        assert_eq!(
            t.on_sample(live(100.0, 225.0, 72.5)),
            Step::Rollback(Reason::OverTemperature),
            "the user's lower ceiling is honoured"
        );

        let mut capped = Tuner::new(SIM_STOCK, 95.0, None);
        into_trial(&mut capped);
        assert_eq!(
            capped.on_sample(live(100.0, 225.0, TEMP_CEILING_C + 0.5)),
            Step::Rollback(Reason::OverTemperature),
            "a ceiling of 95 is still capped at the tuner's own limit"
        );
    }

    #[test]
    fn two_collapsed_samples_in_a_row_roll_back_and_one_does_not() {
        let mut t = Tuner::new(SIM_STOCK, TEMP_CEILING_C, None);
        into_trial(&mut t);
        assert_eq!(t.on_sample(live(40.0, 225.0, 71.0)), Step::Hold);
        assert_eq!(
            t.on_sample(stock_reading()),
            Step::Hold,
            "the streak resets"
        );
        assert_eq!(t.on_sample(live(40.0, 225.0, 71.0)), Step::Hold);
        assert_eq!(
            t.on_sample(live(40.0, 225.0, 71.0)),
            Step::Rollback(Reason::HashrateCollapse)
        );
    }

    #[test]
    fn a_collapse_after_settling_rolls_back_too() {
        let (mut tuner, _, _) = run_sim(None);
        assert_eq!(tuner.phase(), Phase::Tuned);
        let low = live(40.0, 150.0, 60.0);
        assert_eq!(tuner.on_sample(low), Step::Hold);
        assert_eq!(
            tuner.on_sample(low),
            Step::Rollback(Reason::HashrateCollapse)
        );
    }

    #[test]
    fn a_refused_write_and_a_user_stop_both_roll_back_once() {
        let mut t = Tuner::new(SIM_STOCK, TEMP_CEILING_C, None);
        into_trial(&mut t);
        assert_eq!(
            t.stop(Reason::ControlError),
            Step::Rollback(Reason::ControlError)
        );
        assert_eq!(t.stop(Reason::AppExit), Step::Hold, "already rolled back");

        let mut u = Tuner::new(SIM_STOCK, TEMP_CEILING_C, None);
        assert_eq!(
            u.stop(Reason::UserTurnedOff),
            Step::Rollback(Reason::UserTurnedOff)
        );
    }

    #[test]
    fn the_sim_card_refuses_what_it_cannot_do() {
        let mut sim = SimGpu::new();
        let above = Knobs {
            power_w: SIM_STOCK.power_w + 1.0,
            ..SIM_STOCK
        };
        assert!(sim.apply(above).is_err());
        assert!(sim.log.last().is_some_and(|l| l.starts_with("SIM refused")));
    }
}
