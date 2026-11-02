//! Supervision rules for one miner worker, as a pure state machine: restart
//! with backoff, stall and crash-loop limits, and the thermal stop (ADR-073
//! decision 3). No process and no clock: the caller passes `now_ms`, so every
//! rule is tested without a miner running.

use std::collections::VecDeque;

/// First restart delay after an exit; each later exit in the window doubles it.
pub const BACKOFF_FIRST_MS: u64 = 2_000;
/// Longest restart delay.
pub const BACKOFF_CAP_MS: u64 = 300_000;
/// Exits this many times inside [`CRASH_WINDOW_MS`] stop the worker for good.
pub const CRASH_LIMIT: usize = 5;
pub const CRASH_WINDOW_MS: u64 = 600_000;
/// No event from the miner for this long, once it has reported a hash rate,
/// is a stall. The miner reports `hashrate` every two seconds.
pub const STALL_MS: u64 = 60_000;
/// Default GPU temperature ceiling (°C). A default, labelled as one in the UI.
pub const THERMAL_CEILING_C: f64 = 80.0;
/// The worker restarts once the GPU is this far below the ceiling.
pub const THERMAL_HYSTERESIS_C: f64 = 5.0;

/// What the supervisor decides after a worker stops on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterExit {
    /// Start the worker again at this time.
    RestartAt(u64),
    /// Too many exits in the window: stay stopped and alert.
    GiveUp,
}

/// A thermal reading's effect on a running worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thermal {
    /// Nothing to do.
    Unchanged,
    /// Above the ceiling: stop the worker.
    Stop,
    /// Cooled below the resume line: the worker may start again.
    Resume,
}

/// Restart bookkeeping for one worker.
#[derive(Debug, Default)]
pub struct Supervisor {
    exits: VecDeque<u64>,
    restart_at: Option<u64>,
    thermal_held: bool,
}

/// Delay before the `attempt`-th restart in the window (attempt 1 = first exit).
#[must_use]
pub fn backoff_ms(attempt: u32) -> u64 {
    let shift = attempt.saturating_sub(1).min(31);
    BACKOFF_FIRST_MS.saturating_mul(1 << shift).min(BACKOFF_CAP_MS)
}

/// True when the miner has been silent for [`STALL_MS`].
#[must_use]
pub fn stalled(last_event_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_event_ms) >= STALL_MS
}

impl Supervisor {
    /// Records an exit and decides whether to restart it.
    pub fn on_exit(&mut self, now_ms: u64) -> AfterExit {
        let horizon = now_ms.saturating_sub(CRASH_WINDOW_MS);
        while self.exits.front().is_some_and(|&t| t < horizon) {
            self.exits.pop_front();
        }
        self.exits.push_back(now_ms);
        if self.exits.len() >= CRASH_LIMIT {
            self.restart_at = None;
            return AfterExit::GiveUp;
        }
        let at = now_ms.saturating_add(backoff_ms(u32::try_from(self.exits.len()).unwrap_or(u32::MAX)));
        self.restart_at = Some(at);
        AfterExit::RestartAt(at)
    }

    /// True once a scheduled restart is due and the GPU is not held hot.
    /// Taking it clears the schedule.
    pub fn take_due(&mut self, now_ms: u64) -> bool {
        if self.thermal_held || self.restart_at.is_none_or(|at| at > now_ms) {
            return false;
        }
        self.restart_at = None;
        true
    }

    /// Applies one temperature reading. A missing reading changes nothing.
    pub fn on_thermal(&mut self, temp_c: Option<f64>, ceiling_c: f64, now_ms: u64) -> Thermal {
        let Some(temp) = temp_c else {
            return Thermal::Unchanged;
        };
        if !self.thermal_held && temp > ceiling_c {
            self.thermal_held = true;
            return Thermal::Stop;
        }
        if self.thermal_held && temp < ceiling_c - THERMAL_HYSTERESIS_C {
            self.thermal_held = false;
            self.restart_at = Some(now_ms);
            return Thermal::Resume;
        }
        Thermal::Unchanged
    }

    /// A deliberate stop (the user, the window closing) forgets the history.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub fn thermal_held(&self) -> bool {
        self.thermal_held
    }

    #[cfg(test)]
    #[must_use]
    pub fn restart_at(&self) -> Option<u64> {
        self.restart_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_from_two_seconds_and_caps_at_five_minutes() {
        let seq: Vec<u64> = (1..=10).map(backoff_ms).collect();
        assert_eq!(
            seq,
            [2_000, 4_000, 8_000, 16_000, 32_000, 64_000, 128_000, 256_000, 300_000, 300_000]
        );
        assert_eq!(backoff_ms(u32::MAX), BACKOFF_CAP_MS);
        assert_eq!(backoff_ms(0), BACKOFF_FIRST_MS, "attempt 0 is not a shift underflow");
    }

    #[test]
    fn restarts_follow_the_backoff_then_the_crash_limit_gives_up() {
        let mut sup = Supervisor::default();
        let mut t = 1_000;
        let mut restarts = Vec::new();
        for _ in 0..(CRASH_LIMIT - 1) {
            let AfterExit::RestartAt(at) = sup.on_exit(t) else {
                panic!("gave up too early");
            };
            restarts.push(at - t);
            t = at;
            assert!(!sup.take_due(t - 1));
            assert!(sup.take_due(t));
        }
        assert_eq!(restarts, [2_000, 4_000, 8_000, 16_000]);
        assert_eq!(sup.on_exit(t + 1), AfterExit::GiveUp, "fifth exit in the window");
        assert_eq!(sup.restart_at(), None);
    }

    #[test]
    fn old_exits_leave_the_window_so_a_rare_crash_does_not_stop_the_worker() {
        let mut sup = Supervisor::default();
        for i in 0..4 {
            assert!(matches!(sup.on_exit(i * 1_000), AfterExit::RestartAt(_)));
        }
        let later = CRASH_WINDOW_MS + 10_000;
        assert_eq!(sup.on_exit(later), AfterExit::RestartAt(later + BACKOFF_FIRST_MS));
    }

    #[test]
    fn thermal_stop_and_resume_with_hysteresis() {
        let mut sup = Supervisor::default();
        let ceiling = THERMAL_CEILING_C;
        let seq = [79.0, 81.0, 79.0, 75.0, 74.0];
        let got: Vec<Thermal> = seq
            .iter()
            .enumerate()
            .map(|(i, &c)| sup.on_thermal(Some(c), ceiling, i as u64))
            .collect();
        assert_eq!(
            got,
            [
                Thermal::Unchanged,
                Thermal::Stop,
                Thermal::Unchanged,
                Thermal::Unchanged,
                Thermal::Resume,
            ]
        );
    }

    #[test]
    fn a_missing_reading_never_stops_or_resumes() {
        let mut sup = Supervisor::default();
        assert_eq!(sup.on_thermal(None, 80.0, 0), Thermal::Unchanged);
        sup.on_thermal(Some(95.0), 80.0, 0);
        assert_eq!(sup.on_thermal(None, 80.0, 1), Thermal::Unchanged);
        assert!(sup.thermal_held(), "still held after a blank reading");
    }

    #[test]
    fn a_thermal_hold_blocks_a_due_restart_until_the_gpu_cools() {
        let mut sup = Supervisor::default();
        sup.on_thermal(Some(85.0), 80.0, 0);
        sup.restart_at = Some(0);
        assert!(!sup.take_due(10), "held GPU must not restart");
        assert_eq!(sup.on_thermal(Some(76.0), 80.0, 11), Thermal::Unchanged);
        assert_eq!(sup.on_thermal(Some(74.0), 80.0, 12), Thermal::Resume);
        assert!(sup.take_due(12), "cooled: the due restart may run now");
    }

    #[test]
    fn a_stall_is_silence_of_sixty_seconds_or_more() {
        assert!(!stalled(1_000, 1_000 + STALL_MS - 1));
        assert!(stalled(1_000, 1_000 + STALL_MS));
        assert!(!stalled(5_000, 1_000), "a clock that went back is not a stall");
    }

    #[test]
    fn a_deliberate_stop_forgets_the_crash_history_and_any_restart() {
        let mut sup = Supervisor::default();
        for i in 0..4 {
            sup.on_exit(i);
        }
        sup.reset();
        assert_eq!(sup.on_exit(100), AfterExit::RestartAt(100 + BACKOFF_FIRST_MS));
        sup.reset();
        assert_eq!(sup.restart_at(), None);
        assert!(!sup.take_due(u64::MAX), "a stopped worker never restarts");
    }
}
