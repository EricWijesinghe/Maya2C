//! Joules per accepted share, from the power the GPU driver reports.
//!
//! `nvidia-smi power.draw` is the board's own estimate. It is not a wall
//! meter: it leaves out the CPU, the power supply's losses and the rest of
//! the machine, it includes anything else the GPU is drawing for (a desktop,
//! a game), and nobody has checked it against a meter. Every figure from
//! here carries [`LABEL`] so the UI and anything reading the Hub's status
//! shows it that way. How to read it: `docs/benchmarks/hub-energy-per-share.md`.

use serde::Serialize;

/// Shown beside every figure this module produces.
pub const LABEL: &str = "GPU-reported, not wall-measured, unverified";
/// Where the watts come from.
pub const SOURCE: &str = "nvidia-smi power.draw";
/// Longest gap between two power readings that is still integrated. The
/// supervisor reads every 3 s; a longer gap (a failed read, a thermal hold,
/// a stopped process, a clock jump) is left out, and so are the shares
/// accepted inside it, so joules and shares always cover the same time.
pub const MAX_GAP_MS: u64 = 10_000;
const MS_PER_S: f64 = 1_000.0;

/// One power reading and the worker's accepted-share count at that moment.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Reading {
    t_ms: u64,
    watts: f64,
    accepted: u64,
}

/// One GPU worker's metered energy since its session started.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Energy {
    /// Joules over the metered intervals: the trapezoid of consecutive
    /// readings, so a share interval's energy follows the power through it.
    pub joules: f64,
    /// Accepted shares that arrived inside the metered intervals.
    pub shares: u64,
    /// Total length of the metered intervals.
    pub metered_ms: u64,
    /// `joules / shares`; `None` until a share lands in a metered interval.
    pub joules_per_share: Option<f64>,
    pub source: &'static str,
    pub label: &'static str,
    #[serde(skip)]
    last: Option<Reading>,
}

impl Default for Energy {
    fn default() -> Self {
        Self {
            joules: 0.0,
            shares: 0,
            metered_ms: 0,
            joules_per_share: None,
            source: SOURCE,
            label: LABEL,
            last: None,
        }
    }
}

impl Energy {
    /// Folds one reading in. `watts` is `None` when the driver gave no
    /// number or the worker was not hashing, which ends the metered stretch
    /// instead of guessing across it. `accepted` is the worker's running
    /// count of accepted shares.
    pub fn sample(&mut self, t_ms: u64, watts: Option<f64>, accepted: u64) {
        let reading = watts
            .filter(|w| w.is_finite() && *w >= 0.0)
            .map(|watts| Reading {
                t_ms,
                watts,
                accepted,
            });
        if let (Some(prev), Some(now)) = (self.last, reading) {
            let gap = now.t_ms.checked_sub(prev.t_ms);
            if let Some(dt) = gap.filter(|d| (1..=MAX_GAP_MS).contains(d)) {
                #[allow(clippy::cast_precision_loss)] // dt <= MAX_GAP_MS
                let seconds = dt as f64 / MS_PER_S;
                self.joules += (prev.watts + now.watts) / 2.0 * seconds;
                self.shares += now.accepted.saturating_sub(prev.accepted);
                self.metered_ms += dt;
                #[allow(clippy::cast_precision_loss)] // a display figure
                let per = (self.shares > 0).then(|| self.joules / self.shares as f64);
                self.joules_per_share = per;
            }
        }
        self.last = reading;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `(t_ms, watts, accepted)` readings to a fresh meter.
    fn run(readings: &[(u64, Option<f64>, u64)]) -> Energy {
        let mut e = Energy::default();
        for &(t, w, a) in readings {
            e.sample(t, w, a);
        }
        e
    }

    #[test]
    fn steady_power_over_two_shares_is_joules_over_shares() {
        // 200 W for 9 s in three 3 s intervals = 1,800 J; 2 shares inside.
        let e = run(&[
            (0, Some(200.0), 0),
            (3_000, Some(200.0), 1),
            (6_000, Some(200.0), 1),
            (9_000, Some(200.0), 2),
        ]);
        assert_eq!((e.joules, e.shares, e.metered_ms), (1_800.0, 2, 9_000));
        assert_eq!(e.joules_per_share, Some(900.0));
    }

    #[test]
    fn changing_power_is_integrated_as_a_trapezoid() {
        // 100 W rising to 300 W over 2 s: mean 200 W, 400 J, one share.
        let e = run(&[(0, Some(100.0), 0), (2_000, Some(300.0), 1)]);
        assert_eq!(e.joules, 400.0);
        assert_eq!(e.joules_per_share, Some(400.0));
    }

    #[test]
    fn no_share_yet_is_no_figure_not_infinity() {
        let e = run(&[(0, Some(150.0), 0), (3_000, Some(150.0), 0)]);
        assert_eq!(e.joules, 450.0);
        assert_eq!(e.joules_per_share, None);
    }

    #[test]
    fn a_missing_reading_drops_its_interval_and_the_shares_inside_it() {
        // The reading at 3 s is missing: 0..3 and 3..6 are both unmetered,
        // and the share found then is not counted against metered joules.
        let e = run(&[
            (0, Some(200.0), 0),
            (3_000, None, 1),
            (6_000, Some(200.0), 1),
            (9_000, Some(200.0), 2),
        ]);
        assert_eq!((e.joules, e.shares, e.metered_ms), (600.0, 1, 3_000));
        assert_eq!(e.joules_per_share, Some(600.0));
    }

    #[test]
    fn a_long_gap_or_a_clock_step_back_is_not_integrated() {
        let e = run(&[
            (0, Some(200.0), 0),
            (MAX_GAP_MS + 1, Some(200.0), 5),
            (MAX_GAP_MS - 1, Some(200.0), 6),
        ]);
        assert_eq!((e.joules, e.shares, e.metered_ms), (0.0, 0, 0));
        assert_eq!(e.joules_per_share, None);
    }

    #[test]
    fn a_garbage_reading_is_no_reading() {
        let e = run(&[
            (0, Some(200.0), 0),
            (3_000, Some(f64::NAN), 1),
            (6_000, Some(-5.0), 1),
            (9_000, Some(f64::INFINITY), 2),
        ]);
        assert_eq!((e.joules, e.shares), (0.0, 0));
    }

    #[test]
    fn every_figure_carries_the_unverified_label_and_its_source() {
        let json = serde_json::to_value(run(&[(0, Some(1.0), 0)])).expect("serializes");
        assert_eq!(json["label"], "GPU-reported, not wall-measured, unverified");
        assert_eq!(json["source"], "nvidia-smi power.draw");
        assert!(json.get("last").is_none(), "internal state stays internal");
    }
}
