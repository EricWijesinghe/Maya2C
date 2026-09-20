//! Per-channel difficulty control.
//!
//! ## This is the load governor, not a courtesy
//!
//! `docs/stratum-v2.md` §4 does the arithmetic that makes vardiff mandatory
//! here rather than optional: every accepted share costs a 25.4 ms Argon2id
//! pass over 32 MiB. At 50,000 connections, one share per second per channel is
//! 1,270 cores of pure hashing and one share per minute is 21. Nothing else in
//! the pool moves that number.
//!
//! ## Steering on an EWMA of the interval, not on a share count
//!
//! The obvious controller counts shares in a fixed window and retargets on the
//! count. It behaves badly at exactly the moment it matters: a rig that has
//! just connected, or one whose target was just raised, spends the first window
//! with an unrepresentative count and gets retargeted on it. An exponentially
//! weighted mean of the *gaps between shares* has no window boundary to sit
//! inside, and its response to a step change is smooth.
//!
//! ## Why moves are one bit at a time
//!
//! A bit is a factor of two. Larger jumps converge faster on paper and overshoot
//! in practice, because the measurement lags the change: the EWMA is still full
//! of samples taken at the old target when the next decision is made. Single-bit
//! moves with a minimum number of samples between them is the standard, dull,
//! correct answer.
//!
//! ## Backpressure raises targets; it never drops shares
//!
//! [`Vardiff::apply_pressure`] is what the validator pool calls when its queue
//! is filling. Shedding load by dropping submissions would be shedding it onto
//! the miners, who did the work and would not be credited for it. Raising
//! targets sheds the same load onto arithmetic that has not happened yet.

use std::time::Duration;

/// Weight given to the newest interval sample.
///
/// A tenth: roughly a ten-sample memory. Long enough that one lucky share does
/// not move the target, short enough to follow a rig that has actually changed
/// speed.
const EWMA_ALPHA: f64 = 0.1;

/// Accepted shares required between two retunes.
///
/// Without it the controller retunes on the first sample after a change, which
/// is the sample most contaminated by the change itself.
const MIN_SAMPLES_BETWEEN_RETUNES: u32 = 8;

/// How far the observed interval may drift before the target moves.
///
/// A factor of two, matching the granularity of the move itself. A tighter band
/// would retune constantly on the natural variance of a Poisson process, which
/// is what share arrival is.
const RETUNE_BAND: f64 = 2.0;

/// How long a silent channel waits before its target is eased.
///
/// Eight times the configured interval. A channel that should produce a share a
/// minute and has produced none in eight is either much slower than it claimed
/// or was retargeted too hard, and both are fixed the same way.
const IDLE_MULTIPLIER: u32 = 8;

/// One channel's difficulty controller.
#[derive(Clone, Debug)]
pub struct Vardiff {
    /// Interval being steered toward.
    target_interval: Duration,
    /// Lower bound on difficulty, in leading zero bits.
    min_bits: u32,
    /// Upper bound on difficulty, in leading zero bits.
    max_bits: u32,
    /// Current difficulty, in leading zero bits.
    bits: u32,
    /// Exponentially weighted mean gap between accepted shares, in seconds.
    ///
    /// `None` until two shares have arrived: one share gives a timestamp, not
    /// an interval.
    mean_interval: Option<f64>,
    /// When the last accepted share arrived, in milliseconds since the epoch.
    last_share_millis: Option<u64>,
    /// Accepted shares since the last retune.
    samples_since_retune: u32,
}

impl Vardiff {
    /// Starts a controller at `initial_bits`, clamped into the configured band.
    #[must_use]
    pub fn new(initial_bits: u32, min_bits: u32, max_bits: u32, target_interval: Duration) -> Self {
        Self {
            target_interval,
            min_bits,
            max_bits,
            bits: initial_bits.clamp(min_bits, max_bits),
            mean_interval: None,
            last_share_millis: None,
            samples_since_retune: 0,
        }
    }

    /// Current difficulty, in leading zero bits.
    #[must_use]
    pub fn bits(&self) -> u32 {
        self.bits
    }

    /// Observed mean interval between accepted shares, in seconds.
    #[must_use]
    pub fn mean_interval_seconds(&self) -> Option<f64> {
        self.mean_interval
    }

    /// Records an accepted share and returns the new difficulty if it changed.
    ///
    /// `now_millis` is the pool's own clock. The miner's `ntime` is never used
    /// here: a controller steering on a timestamp the miner chose is a
    /// controller the miner can steer.
    pub fn on_accepted_share(&mut self, now_millis: u64) -> Option<u32> {
        if let Some(previous) = self.last_share_millis {
            // Saturating: a clock that stepped backwards yields a zero gap,
            // which the EWMA absorbs. Wrapping would yield an enormous one and
            // drive the target to the floor.
            let gap_seconds = now_millis.saturating_sub(previous) as f64 / 1_000.0;
            self.mean_interval = Some(match self.mean_interval {
                Some(mean) => mean + EWMA_ALPHA * (gap_seconds - mean),
                None => gap_seconds,
            });
            self.samples_since_retune += 1;
        }
        self.last_share_millis = Some(now_millis);

        self.retune()
    }

    /// Eases the target for a channel that has gone quiet.
    ///
    /// Returns the new difficulty if it changed. Called on a timer rather than
    /// on a share, because the failure this fixes is the absence of shares.
    pub fn on_idle_check(&mut self, now_millis: u64) -> Option<u32> {
        let idle_limit = self.target_interval.as_millis() as u64 * u64::from(IDLE_MULTIPLIER);
        let silent_for = {
            let last = self.last_share_millis?;
            now_millis.saturating_sub(last)
        };

        if silent_for < idle_limit || self.bits <= self.min_bits {
            return None;
        }

        self.bits -= 1;
        // The samples that led here were taken at the old target and say
        // nothing about the new one.
        self.mean_interval = None;
        self.samples_since_retune = 0;
        Some(self.bits)
    }

    /// Raises this channel's difficulty because the pool is saturated.
    ///
    /// Returns the new difficulty if it changed. The share rate this sheds is
    /// work the pool would otherwise have to verify; the alternative — dropping
    /// submissions — sheds work miners already did.
    pub fn apply_pressure(&mut self) -> Option<u32> {
        if self.bits >= self.max_bits {
            return None;
        }
        self.bits += 1;
        self.mean_interval = None;
        self.samples_since_retune = 0;
        Some(self.bits)
    }

    /// Moves one bit if the observed interval has drifted outside the band.
    fn retune(&mut self) -> Option<u32> {
        if self.samples_since_retune < MIN_SAMPLES_BETWEEN_RETUNES {
            return None;
        }
        let mean = self.mean_interval?;
        let target = self.target_interval.as_secs_f64();

        let new_bits = if mean * RETUNE_BAND < target {
            // Shares are arriving too fast: make the target harder.
            self.bits + 1
        } else if mean > target * RETUNE_BAND {
            // Too slow: make it easier.
            self.bits.saturating_sub(1)
        } else {
            return None;
        }
        .clamp(self.min_bits, self.max_bits);

        if new_bits == self.bits {
            return None;
        }

        self.bits = new_bits;
        self.mean_interval = None;
        self.samples_since_retune = 0;
        Some(self.bits)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const INTERVAL: Duration = Duration::from_secs(60);

    fn controller() -> Vardiff {
        Vardiff::new(20, 16, 40, INTERVAL)
    }

    /// Feeds `count` shares spaced `gap_millis` apart, starting at `start`.
    fn feed(vardiff: &mut Vardiff, start: u64, gap_millis: u64, count: u32) -> u64 {
        let mut now = start;
        for _ in 0..count {
            now += gap_millis;
            vardiff.on_accepted_share(now);
        }
        now
    }

    #[test]
    fn a_fast_channel_is_made_harder() {
        let mut vardiff = controller();
        // A share a second against a sixty-second goal.
        feed(&mut vardiff, 0, 1_000, 40);
        assert!(
            vardiff.bits() > 20,
            "target did not tighten: {} bits",
            vardiff.bits()
        );
    }

    #[test]
    fn a_slow_channel_is_made_easier() {
        let mut vardiff = controller();
        feed(&mut vardiff, 0, 600_000, 40);
        assert!(
            vardiff.bits() < 20,
            "target did not loosen: {} bits",
            vardiff.bits()
        );
    }

    #[test]
    fn a_channel_at_the_target_interval_is_left_alone() {
        let mut vardiff = controller();
        feed(&mut vardiff, 0, INTERVAL.as_millis() as u64, 60);
        assert_eq!(vardiff.bits(), 20);
    }

    #[test]
    fn the_first_share_cannot_retune() {
        // One share is a timestamp, not an interval. A controller that acted on
        // it would act on nothing.
        let mut vardiff = controller();
        assert_eq!(vardiff.on_accepted_share(1_000), None);
        assert_eq!(vardiff.bits(), 20);
    }

    #[test]
    fn difficulty_stays_inside_the_configured_band() {
        let mut vardiff = Vardiff::new(17, 16, 18, INTERVAL);
        feed(&mut vardiff, 0, 1, 500);
        assert!(vardiff.bits() <= 18);

        let mut vardiff = Vardiff::new(17, 16, 18, INTERVAL);
        feed(&mut vardiff, 0, 3_600_000, 500);
        assert!(vardiff.bits() >= 16);
    }

    #[test]
    fn a_backwards_clock_step_does_not_drive_the_target_to_the_floor() {
        // Wrapping subtraction here would produce a gap of ~584 million years
        // and one share would empty the band.
        let mut vardiff = controller();
        vardiff.on_accepted_share(10_000);
        for _ in 0..MIN_SAMPLES_BETWEEN_RETUNES + 2 {
            vardiff.on_accepted_share(5_000);
        }
        assert!(vardiff.bits() >= 16);
    }

    #[test]
    fn a_silent_channel_is_eased_after_the_idle_window() {
        let mut vardiff = controller();
        vardiff.on_accepted_share(0);

        let idle = INTERVAL.as_millis() as u64 * u64::from(IDLE_MULTIPLIER);
        assert_eq!(vardiff.on_idle_check(idle - 1), None);
        assert_eq!(vardiff.on_idle_check(idle + 1), Some(19));
    }

    #[test]
    fn pressure_raises_the_target_and_stops_at_the_ceiling() {
        let mut vardiff = Vardiff::new(39, 16, 40, INTERVAL);
        assert_eq!(vardiff.apply_pressure(), Some(40));
        // Shares are still never dropped; the pool simply cannot ask for less
        // work than the ceiling allows.
        assert_eq!(vardiff.apply_pressure(), None);
    }

    #[test]
    fn a_retune_discards_the_samples_that_caused_it() {
        // Samples taken at the old target describe the old target. Keeping them
        // is what makes a controller oscillate.
        let mut vardiff = controller();
        let mut now = 0;

        loop {
            now += 1_000;
            if vardiff.on_accepted_share(now).is_some() {
                break;
            }
            assert!(now < 1_000_000, "the controller never retuned");
        }

        assert_eq!(vardiff.mean_interval_seconds(), None);
    }
}
