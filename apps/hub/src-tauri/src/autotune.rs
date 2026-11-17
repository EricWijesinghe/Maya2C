//! Hub GPU tuner, the decision core (ADR-137 HW-1, stage T1a: guided).
//!
//! The Hub never writes to the card. This state machine says which power
//! limit the user should set next, waits until the driver reads that limit
//! back, measures hash rate per watt over a window, and stops at the first
//! step whose efficiency is no better than the previous one or whose hash rate
//! is under the user's floor; it keeps the previous step. Power limit only
//! (O-1): no clocks, no fans.
//!
//! Pure: no I/O, no clock, no randomness. `tuning.rs` feeds it one [`Sample`]
//! per supervisor tick, so the same samples always give the same steps.

use serde::Serialize;

use crate::watchdog::{THERMAL_CEILING_C, THERMAL_HYSTERESIS_C};

// Policy values, not measurements (ADR-137: chosen from measurement in a
// later PR; none exists yet). Changing one needs a recorded run in
// docs/mining/gpu-autotune.md.
/// The hash-rate floor slider's default, in percent of the default-limit rate (O-3).
pub const FLOOR_PCT_DEFAULT: f64 = 95.0;
/// The slider's range. The user moves inside it and never outside.
pub const FLOOR_PCT_MIN: f64 = 80.0;
pub const FLOOR_PCT_MAX: f64 = 100.0;
/// One step down, as a share of the driver's range (default minus minimum).
pub const STEP_FRAC: f64 = 0.1;
/// Samples ignored after the limit reads back, so the card can settle. At
/// the supervisor's 3-second GPU tick this is 30 s.
pub const SETTLE_SAMPLES: u32 = 10;
/// Valid samples averaged into one measurement: 60 s at a 3-second tick.
pub const WINDOW_SAMPLES: usize = 20;
/// How far a read-back limit may sit from the asked value (the driver rounds).
pub const READBACK_TOLERANCE_W: f64 = 1.0;
/// A trial aborts this far below the user's temperature ceiling.
pub const THERMAL_MARGIN_C: f64 = 3.0;

/// The driver-reported power range for one card in this session.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Range {
    pub min_w: f64,
    pub default_w: f64,
}

impl Range {
    /// `None` unless the driver reported both ends, finite, with `0 < min < default`.
    #[must_use]
    pub fn new(min_w: Option<f64>, default_w: Option<f64>) -> Option<Self> {
        let (min_w, default_w) = (min_w?, default_w?);
        (min_w.is_finite() && default_w.is_finite() && min_w > 0.0 && min_w < default_w)
            .then_some(Self { min_w, default_w })
    }

    fn step_w(self) -> f64 {
        ((self.default_w - self.min_w) * STEP_FRAC).max(1.0)
    }
}

/// Hash rate per watt (hashes per joule), or `None` for a reading that is
/// missing, zero or not finite. The efficiency read-out (ADR-137 T0).
#[must_use]
pub fn efficiency(hps: Option<f64>, power_w: Option<f64>) -> Option<f64> {
    let (hps, power_w) = (hps?, power_w?);
    (hps.is_finite() && power_w.is_finite() && hps > 0.0 && power_w > 0.0).then(|| hps / power_w)
}

/// One reading of a card and its miner. `None` is a reading the driver or
/// the miner did not give: never a guess.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub hps: Option<f64>,
    pub power_w: Option<f64>,
    /// The power limit the driver reads back now.
    pub limit_w: Option<f64>,
    pub temp_c: Option<f64>,
    pub running: bool,
    pub restarts: u32,
    pub rejected: u64,
    pub hw_errors: u64,
}

/// Why a session stopped early.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Reason {
    MissingReading,
    MinerStopped,
    MinerRestarted,
    OverTemperature,
    HardwareError,
    SharesRejected,
    LimitAboveDefault,
    JournalFailed,
    UserStopped,
}

impl Reason {
    /// Every reason, for the tests.
    #[cfg(test)]
    pub const ALL: [Self; 9] = [
        Self::MissingReading,
        Self::MinerStopped,
        Self::MinerRestarted,
        Self::OverTemperature,
        Self::HardwareError,
        Self::SharesRejected,
        Self::LimitAboveDefault,
        Self::JournalFailed,
        Self::UserStopped,
    ];

    #[must_use]
    pub const fn words(self) -> &'static str {
        match self {
            Self::MissingReading => "a reading went missing",
            Self::MinerStopped => "the miner stopped",
            Self::MinerRestarted => "the miner crashed and restarted",
            Self::OverTemperature => "the card reached its temperature limit",
            Self::HardwareError => "the card returned a wrong hash",
            Self::SharesRejected => "more shares were rejected than at the default",
            Self::LimitAboveDefault => "the power limit is above the driver default",
            Self::JournalFailed => "the Hub could not record the step",
            Self::UserStopped => "you stopped tuning",
        }
    }

    /// True when the stop says something about the trial setting, so that
    /// step is marked bad for this card and driver.
    #[must_use]
    pub const fn implicates_setting(self) -> bool {
        matches!(
            self,
            Self::MinerRestarted
                | Self::OverTemperature
                | Self::HardwareError
                | Self::SharesRejected
        )
    }
}

/// What the user should do after a stop. Always back to the driver
/// default; on heat, only once the card has cooled, because the default is
/// a higher limit than the trial (ADR-137: an abort never raises the limit
/// on a hot card).
#[must_use]
pub fn rollback_guidance(reason: Reason, range: Range, ceiling_c: f64) -> String {
    let d = range.default_w.round();
    match reason {
        Reason::OverTemperature => format!(
            "Stopped: {}. Leave the power limit where it is until the card reads below {:.0} °C, \
             then set it back to the default, {d} W.",
            reason.words(),
            capped_ceiling(ceiling_c) - THERMAL_HYSTERESIS_C
        ),
        _ => format!(
            "Stopped: {}. Set the power limit back to the default, {d} W.",
            reason.words()
        ),
    }
}

fn capped_ceiling(ceiling_c: f64) -> f64 {
    if ceiling_c.is_finite() {
        ceiling_c.min(THERMAL_CEILING_C)
    } else {
        THERMAL_CEILING_C
    }
}

/// The slider value, kept inside its compiled range. A value that is not a
/// number is the default.
#[must_use]
pub fn clamp_floor_pct(pct: f64) -> f64 {
    if pct.is_finite() {
        pct.clamp(FLOOR_PCT_MIN, FLOOR_PCT_MAX)
    } else {
        FLOOR_PCT_DEFAULT
    }
}

/// Where a session is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum Phase {
    /// Waiting for a hashing miner with the limit at the driver default.
    WaitDefault,
    /// Measuring at the default: the reference for every step.
    Baseline,
    /// The user was asked to set this limit; waiting to read it back.
    Await(f64),
    /// The limit reads back; settling, then measuring.
    Measure(f64),
    /// Finished: keep this limit.
    Done(f64),
    Aborted(Reason),
}

impl Phase {
    /// True until the search is finished or stopped.
    #[must_use]
    pub const fn active(self) -> bool {
        !matches!(self, Self::Done(_) | Self::Aborted(_))
    }
}

/// What the driver must record after a sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Hold,
    /// A new step to show. The journal entry is written before it is shown.
    Suggest {
        trial_w: f64,
        previous_w: f64,
    },
    Finished {
        best_w: f64,
        gain_pct: f64,
    },
    Aborted(Reason),
}

/// One card's guided search.
pub struct Tuner {
    range: Range,
    floor: f64,
    ceiling_c: f64,
    /// Steps at or below this limit were marked bad for this driver.
    bad_at_or_below_w: Option<f64>,
    phase: Phase,
    settle_left: u32,
    window: Vec<(f64, f64)>,
    base_hps: f64,
    base_eff: f64,
    prev_w: f64,
    prev_eff: f64,
    start_restarts: Option<u32>,
    start_hw_errors: u64,
    rejected_at_window: u64,
    base_rejected: u64,
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= READBACK_TOLERANCE_W
}

impl Tuner {
    /// `floor_pct` is the slider; `ceiling_c` the user's temperature ceiling,
    /// capped at the watchdog's own.
    #[must_use]
    pub fn new(
        range: Range,
        floor_pct: f64,
        ceiling_c: f64,
        bad_at_or_below_w: Option<f64>,
    ) -> Self {
        Self {
            range,
            floor: clamp_floor_pct(floor_pct) / 100.0,
            ceiling_c: capped_ceiling(ceiling_c),
            bad_at_or_below_w,
            phase: Phase::WaitDefault,
            settle_left: 0,
            window: Vec::new(),
            base_hps: 0.0,
            base_eff: 0.0,
            prev_w: range.default_w,
            prev_eff: 0.0,
            start_restarts: None,
            start_hw_errors: 0,
            rejected_at_window: 0,
            base_rejected: 0,
        }
    }

    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    #[must_use]
    pub const fn range(&self) -> Range {
        self.range
    }

    #[must_use]
    pub const fn ceiling_c(&self) -> f64 {
        self.ceiling_c
    }

    #[must_use]
    pub fn floor_pct(&self) -> f64 {
        self.floor * 100.0
    }

    /// Ends the session from outside (the user, or a failed journal write).
    pub fn abort(&mut self, reason: Reason) -> Step {
        if self.phase.active() {
            self.phase = Phase::Aborted(reason);
            Step::Aborted(reason)
        } else {
            Step::Hold
        }
    }

    pub fn on_sample(&mut self, s: Sample) -> Step {
        match self.phase {
            Phase::Done(_) | Phase::Aborted(_) => Step::Hold,
            Phase::WaitDefault => self.wait_default(s),
            _ => match self.safety(s) {
                Some(reason) => self.abort(reason),
                None => self.progress(s),
            },
        }
    }

    fn wait_default(&mut self, s: Sample) -> Step {
        let ready = s.running
            && s.limit_w.is_some_and(|l| near(l, self.range.default_w))
            && efficiency(s.hps, s.power_w).is_some();
        if ready {
            self.phase = Phase::Baseline;
            self.settle_left = SETTLE_SAMPLES;
            self.start_restarts = Some(s.restarts);
            self.start_hw_errors = s.hw_errors;
            self.rejected_at_window = s.rejected;
            self.window.clear();
        }
        Step::Hold
    }

    /// The abort checks, run on every sample once measuring has begun.
    fn safety(&self, s: Sample) -> Option<Reason> {
        if !s.running {
            return Some(Reason::MinerStopped);
        }
        if self.start_restarts.is_some_and(|r| s.restarts > r) {
            return Some(Reason::MinerRestarted);
        }
        if s.hw_errors > self.start_hw_errors {
            return Some(Reason::HardwareError);
        }
        let (Some(_), Some(_), Some(limit), Some(temp)) = (s.hps, s.power_w, s.limit_w, s.temp_c)
        else {
            return Some(Reason::MissingReading);
        };
        if !(limit.is_finite() && temp.is_finite()) {
            return Some(Reason::MissingReading);
        }
        if temp >= self.ceiling_c - THERMAL_MARGIN_C {
            return Some(Reason::OverTemperature);
        }
        if limit > self.range.default_w + READBACK_TOLERANCE_W {
            return Some(Reason::LimitAboveDefault);
        }
        let in_window = s.rejected.saturating_sub(self.rejected_at_window);
        (matches!(self.phase, Phase::Measure(_)) && in_window > self.base_rejected)
            .then_some(Reason::SharesRejected)
    }

    fn progress(&mut self, s: Sample) -> Step {
        let limit = s.limit_w.unwrap_or(f64::NAN);
        match self.phase {
            Phase::Baseline if !near(limit, self.range.default_w) => {
                self.phase = Phase::WaitDefault;
                Step::Hold
            }
            Phase::Await(w) if near(limit, w) => {
                self.phase = Phase::Measure(w);
                self.settle_left = SETTLE_SAMPLES;
                self.window.clear();
                self.rejected_at_window = s.rejected;
                Step::Hold
            }
            Phase::Measure(w) if !near(limit, w) => {
                // The user moved it: nothing is measured until it reads back again.
                self.phase = Phase::Await(w);
                self.window.clear();
                Step::Hold
            }
            Phase::Baseline | Phase::Measure(_) => self.measure(s),
            _ => Step::Hold,
        }
    }

    /// Adds a sample to the window. A zero, missing or non-finite sample
    /// voids the window, so a step needs a full valid one.
    fn measure(&mut self, s: Sample) -> Step {
        if self.settle_left > 0 {
            self.settle_left -= 1;
            return Step::Hold;
        }
        let (Some(hps), Some(power)) = (s.hps, s.power_w) else {
            self.window.clear();
            return Step::Hold;
        };
        if efficiency(Some(hps), Some(power)).is_none() {
            self.window.clear();
            return Step::Hold;
        }
        self.window.push((hps, power));
        if self.window.len() < WINDOW_SAMPLES {
            return Step::Hold;
        }
        let n = self.window.len() as f64;
        let (h, p) = self
            .window
            .drain(..)
            .fold((0.0, 0.0), |a, (h, p)| (a.0 + h, a.1 + p));
        let (hps, eff) = (h / n, h / p);
        let rejected_now = s.rejected.saturating_sub(self.rejected_at_window);
        self.rejected_at_window = s.rejected;
        match self.phase {
            Phase::Measure(w) => self.judge(w, hps, eff),
            _ => {
                self.base_hps = hps;
                self.base_eff = eff;
                self.prev_eff = eff;
                self.base_rejected = rejected_now;
                self.next_from(self.range.default_w)
            }
        }
    }

    fn judge(&mut self, w: f64, hps: f64, eff: f64) -> Step {
        if hps < self.floor * self.base_hps || eff <= self.prev_eff {
            return self.finish(self.prev_w);
        }
        self.prev_w = w;
        self.prev_eff = eff;
        self.next_from(w)
    }

    /// The next step below `from`, or the end of the search.
    fn next_from(&mut self, from: f64) -> Step {
        let next = (from - self.range.step_w())
            .round()
            .max(self.range.min_w.ceil());
        let allowed = next < from
            && next >= self.range.min_w
            && next < self.range.default_w
            && self.bad_at_or_below_w.is_none_or(|bad| next > bad);
        if !allowed {
            return self.finish(from);
        }
        self.phase = Phase::Await(next);
        Step::Suggest {
            trial_w: next,
            previous_w: from,
        }
    }

    fn finish(&mut self, best_w: f64) -> Step {
        self.phase = Phase::Done(best_w);
        let gain_pct = if self.base_eff > 0.0 && best_w < self.range.default_w {
            (self.prev_eff / self.base_eff - 1.0) * 100.0
        } else {
            0.0
        };
        Step::Finished { best_w, gain_pct }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_gpu::{SIM_RANGE, SimGpu};

    const CEILING: f64 = 80.0;

    /// Drives one tuner against the SIM card, the test playing the user who
    /// sets each suggested limit. Returns every suggestion and the end step.
    fn run_sim(floor_pct: f64) -> (Vec<f64>, Step, SimGpu) {
        const RUN_CAP: usize = 2_000;
        let mut sim = SimGpu::new();
        let mut t = Tuner::new(SIM_RANGE, floor_pct, CEILING, None);
        let mut asked = Vec::new();
        for _ in 0..RUN_CAP {
            match t.on_sample(sim.sample()) {
                Step::Suggest { trial_w, .. } => {
                    asked.push(trial_w);
                    sim.set_limit(trial_w);
                }
                step @ (Step::Finished { .. } | Step::Aborted(_)) => return (asked, step, sim),
                Step::Hold => {}
            }
        }
        panic!("the search must end inside {RUN_CAP} samples");
    }

    /// A tuner moved into `Measure` of its first step, with a steady reading.
    fn into_measure() -> (Tuner, SimGpu) {
        let mut sim = SimGpu::new();
        let mut t = Tuner::new(SIM_RANGE, FLOOR_PCT_DEFAULT, CEILING, None);
        for _ in 0..200 {
            if let Step::Suggest { trial_w, .. } = t.on_sample(sim.sample()) {
                sim.set_limit(trial_w);
                t.on_sample(sim.sample());
                assert!(matches!(t.phase(), Phase::Measure(_)));
                return (t, sim);
            }
        }
        panic!("no first step");
    }

    #[test]
    fn no_step_ever_exceeds_the_default_limit_or_leaves_the_driver_range() {
        for floor in [FLOOR_PCT_MIN, 85.0, 90.0, FLOOR_PCT_DEFAULT, FLOOR_PCT_MAX] {
            let (asked, end, _) = run_sim(floor);
            for w in &asked {
                assert!(
                    *w < SIM_RANGE.default_w && *w >= SIM_RANGE.min_w,
                    "{w} W at floor {floor}"
                );
            }
            let Step::Finished { best_w, .. } = end else {
                panic!("{end:?}");
            };
            assert!((SIM_RANGE.min_w..=SIM_RANGE.default_w).contains(&best_w));
        }
    }

    #[test]
    fn the_search_stops_at_the_hash_rate_floor_and_keeps_the_previous_step() {
        // Hand-derived for the SIM curve (sim_gpu.rs): draw is 0.9 x limit and
        // hash rate follows draw^0.3, so efficiency always rises as the limit
        // falls and only the floor stops the walk. Steps are 15 W.
        // Floor 95 %: 205 W gives 94.2 % of the default rate, so 220 W is kept.
        // Floor 90 %: 175 W gives 89.8 %, so 190 W is kept.
        for (floor, best) in [(95.0, 220.0), (90.0, 190.0)] {
            let (asked, end, sim) = run_sim(floor);
            assert_eq!(
                end,
                Step::Finished {
                    best_w: best,
                    gain_pct: end_gain(end)
                }
            );
            assert!(
                asked.contains(&(best - 15.0)),
                "the step past the floor was tried"
            );
            let kept = sim.rate_at(best) / sim.rate_at(SIM_RANGE.default_w) * 100.0;
            assert!(kept >= floor, "{kept:.1}% kept at floor {floor}");
        }
    }

    fn end_gain(step: Step) -> f64 {
        match step {
            Step::Finished { gain_pct, .. } => gain_pct,
            _ => f64::NAN,
        }
    }

    #[test]
    fn the_floor_slider_is_respected_and_kept_inside_its_range() {
        assert_eq!(clamp_floor_pct(10.0), FLOOR_PCT_MIN);
        assert_eq!(clamp_floor_pct(150.0), FLOOR_PCT_MAX);
        assert_eq!(clamp_floor_pct(f64::NAN), FLOOR_PCT_DEFAULT);
        assert_eq!(clamp_floor_pct(92.0), 92.0);
        // At 100 % the first step already loses hash rate, so the default is kept.
        let (_, end, _) = run_sim(FLOOR_PCT_MAX);
        assert_eq!(
            end,
            Step::Finished {
                best_w: SIM_RANGE.default_w,
                gain_pct: 0.0
            }
        );
        // A lower floor walks further down, never less far.
        let (low, _, _) = run_sim(FLOOR_PCT_MIN);
        let (high, _, _) = run_sim(FLOOR_PCT_DEFAULT);
        assert!(low.len() > high.len());
    }

    #[test]
    fn the_search_is_deterministic() {
        assert_eq!(run_sim(90.0).0, run_sim(90.0).0);
    }

    #[test]
    fn the_search_stops_when_efficiency_falls_and_keeps_the_previous_step() {
        let mut t = Tuner::new(SIM_RANGE, FLOOR_PCT_MIN, CEILING, None);
        let at = |limit: f64, hps: f64, power: f64| Sample {
            hps: Some(hps),
            power_w: Some(power),
            limit_w: Some(limit),
            temp_c: Some(60.0),
            running: true,
            ..Sample::default()
        };
        let feed = |t: &mut Tuner, s: Sample| {
            (0..=SETTLE_SAMPLES as usize + WINDOW_SAMPLES)
                .map(|_| t.on_sample(s))
                .find(|s| *s != Step::Hold)
        };
        assert_eq!(
            feed(&mut t, at(250.0, 100.0, 200.0)),
            Some(Step::Suggest {
                trial_w: 235.0,
                previous_w: 250.0
            })
        );
        t.on_sample(at(235.0, 99.0, 180.0));
        assert_eq!(
            feed(&mut t, at(235.0, 99.0, 180.0)),
            Some(Step::Suggest {
                trial_w: 220.0,
                previous_w: 235.0
            }),
            "0.55 H/J beats 0.50"
        );
        t.on_sample(at(220.0, 90.0, 175.0));
        let end = feed(&mut t, at(220.0, 90.0, 175.0));
        assert!(
            matches!(end, Some(Step::Finished { best_w, .. }) if best_w == 235.0),
            "0.514 H/J is worse than 0.55, so 235 W is kept: {end:?}"
        );
    }

    #[test]
    fn rollback_guidance_shows_on_each_trigger() {
        for reason in Reason::ALL {
            let (mut t, sim) = into_measure();
            let mut s = sim.sample();
            match reason {
                Reason::MissingReading => s.temp_c = None,
                Reason::MinerStopped => s.running = false,
                Reason::MinerRestarted => s.restarts += 1,
                Reason::OverTemperature => s.temp_c = Some(CEILING - THERMAL_MARGIN_C),
                Reason::HardwareError => s.hw_errors += 1,
                Reason::SharesRejected => s.rejected += 1,
                Reason::LimitAboveDefault => s.limit_w = Some(SIM_RANGE.default_w + 10.0),
                Reason::JournalFailed | Reason::UserStopped => {}
            }
            let step = match reason {
                Reason::JournalFailed | Reason::UserStopped => t.abort(reason),
                _ => t.on_sample(s),
            };
            assert_eq!(step, Step::Aborted(reason));
            assert_eq!(t.phase(), Phase::Aborted(reason));
            assert_eq!(
                t.on_sample(sim.sample()),
                Step::Hold,
                "nothing after a stop"
            );
            let text = rollback_guidance(reason, SIM_RANGE, CEILING);
            assert!(text.contains("250 W"), "{reason:?}: {text}");
            assert!(text.contains(reason.words()), "{reason:?}: {text}");
        }
    }

    #[test]
    fn a_thermal_abort_never_raises_the_limit_while_hot() {
        let text = rollback_guidance(Reason::OverTemperature, SIM_RANGE, 95.0);
        assert!(text.contains("Leave the power limit where it is"), "{text}");
        assert!(
            text.contains(&format!(
                "{:.0} °C",
                THERMAL_CEILING_C - THERMAL_HYSTERESIS_C
            )),
            "a 95 °C ceiling is capped at the watchdog's: {text}"
        );
        let other = rollback_guidance(Reason::MinerStopped, SIM_RANGE, CEILING);
        assert!(!other.contains("Leave the power limit"), "{other}");
    }

    #[test]
    fn a_missing_reading_aborts_the_trial_and_is_never_a_guess() {
        for blank in 0..4 {
            let (mut t, sim) = into_measure();
            let mut s = sim.sample();
            match blank {
                0 => s.hps = None,
                1 => s.power_w = None,
                2 => s.limit_w = None,
                _ => s.temp_c = None,
            }
            assert_eq!(
                t.on_sample(s),
                Step::Aborted(Reason::MissingReading),
                "field {blank}"
            );
        }
    }

    #[test]
    fn a_suggested_limit_that_does_not_read_back_is_not_measured() {
        let sim = SimGpu::new();
        let mut t = Tuner::new(SIM_RANGE, FLOOR_PCT_DEFAULT, CEILING, None);
        let first = (0..200)
            .map(|_| t.on_sample(sim.sample()))
            .find(|s| matches!(s, Step::Suggest { .. }));
        assert!(first.is_some());
        // The user never sets it: the card still reads the default.
        for _ in 0..500 {
            assert_eq!(t.on_sample(sim.sample()), Step::Hold);
        }
        assert_eq!(t.phase(), Phase::Await(235.0));
    }

    #[test]
    fn a_window_with_a_zero_power_or_non_finite_sample_is_void() {
        let (mut t, sim) = into_measure();
        for _ in 0..SETTLE_SAMPLES + WINDOW_SAMPLES as u32 - 1 {
            assert_eq!(t.on_sample(sim.sample()), Step::Hold);
        }
        let mut zero = sim.sample();
        zero.power_w = Some(0.0);
        assert_eq!(
            t.on_sample(zero),
            Step::Hold,
            "the window that would have closed is void"
        );
        let mut nan = sim.sample();
        nan.hps = Some(f64::NAN);
        assert_eq!(t.on_sample(nan), Step::Hold);
        for _ in 0..WINDOW_SAMPLES - 1 {
            assert_eq!(
                t.on_sample(sim.sample()),
                Step::Hold,
                "a full new window is needed"
            );
        }
        assert_ne!(t.on_sample(sim.sample()), Step::Hold);
    }

    #[test]
    fn one_hw_error_event_aborts_the_trial() {
        let (mut t, sim) = into_measure();
        let mut s = sim.sample();
        s.hw_errors = 1;
        assert_eq!(t.on_sample(s), Step::Aborted(Reason::HardwareError));
        assert!(Reason::HardwareError.implicates_setting());
        assert!(!Reason::UserStopped.implicates_setting());
    }

    #[test]
    fn measuring_waits_for_a_hashing_miner_at_the_default_limit() {
        let mut sim = SimGpu::new();
        sim.set_limit(200.0);
        let mut t = Tuner::new(SIM_RANGE, FLOOR_PCT_DEFAULT, CEILING, None);
        for _ in 0..100 {
            assert_eq!(t.on_sample(sim.sample()), Step::Hold);
        }
        assert_eq!(
            t.phase(),
            Phase::WaitDefault,
            "a card left at 200 W is not a baseline"
        );
    }

    #[test]
    fn a_step_marked_bad_is_never_suggested_again() {
        let mut sim = SimGpu::new();
        let mut t = Tuner::new(SIM_RANGE, FLOOR_PCT_MIN, CEILING, Some(220.0));
        let mut asked = Vec::new();
        for _ in 0..2_000 {
            match t.on_sample(sim.sample()) {
                Step::Suggest { trial_w, .. } => {
                    asked.push(trial_w);
                    sim.set_limit(trial_w);
                }
                Step::Finished { best_w, .. } => {
                    assert_eq!(best_w, 235.0);
                    break;
                }
                Step::Aborted(r) => panic!("{r:?}"),
                Step::Hold => {}
            }
        }
        assert_eq!(asked, [235.0]);
    }

    #[test]
    fn efficiency_is_hash_rate_per_watt_and_nothing_for_a_bad_reading() {
        assert_eq!(efficiency(Some(100.0), Some(200.0)), Some(0.5));
        assert_eq!(efficiency(Some(100.0), Some(0.0)), None);
        assert_eq!(efficiency(None, Some(200.0)), None);
        assert_eq!(efficiency(Some(f64::INFINITY), Some(200.0)), None);
    }

    #[test]
    fn a_range_needs_both_ends_from_the_driver() {
        assert_eq!(Range::new(Some(100.0), None), None);
        assert_eq!(Range::new(Some(250.0), Some(100.0)), None);
        assert_eq!(Range::new(Some(0.0), Some(250.0)), None);
        assert_eq!(Range::new(Some(100.0), Some(250.0)), Some(SIM_RANGE));
    }
}
