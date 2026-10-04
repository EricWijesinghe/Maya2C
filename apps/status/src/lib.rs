//! Uptime of a chain, measured from the chain itself.
//!
//! Every block header carries a timestamp. A gap between two consecutive
//! blocks longer than the halt threshold is a halt; the sum of those gaps
//! over the time since genesis is the downtime. Nothing here trusts the
//! status service's own clock for the history, so anyone can recompute the
//! same report from the same chain. The live answer to "is it halted now"
//! does use the clock: the gap since the newest block.
//!
//! Not consensus code: floats are fine for a percentage on a status page.

#![warn(missing_docs)]

use serde::Serialize;

/// A gap between consecutive blocks long enough to count as a halt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Halt {
    /// The last block before the gap.
    pub height: u64,
    /// Its timestamp, Unix seconds.
    pub started: u64,
    /// The next block's timestamp, Unix seconds.
    pub ended: u64,
}

impl Halt {
    /// Length of the halt in seconds.
    #[must_use]
    pub const fn seconds(&self) -> u64 {
        self.ended - self.started
    }
}

/// What a status page shows.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    /// Newest block height seen.
    pub height: u64,
    /// Genesis timestamp, Unix seconds.
    pub genesis: u64,
    /// Newest block timestamp, Unix seconds.
    pub last_block: u64,
    /// Fraction of the time since genesis the chain was producing, 0–1.
    pub uptime: f64,
    /// Seconds since the last halt ended (or since genesis if none).
    pub streak_seconds: u64,
    /// No block for longer than the threshold, right now.
    pub halted_now: bool,
    /// Seconds since the newest block.
    pub since_last_block: u64,
    /// Every halt since genesis, oldest first.
    pub halts: Vec<Halt>,
    /// The gap that counts as a halt, seconds.
    pub threshold_seconds: u64,
}

/// Folds block timestamps, in height order, into halts.
#[derive(Clone, Debug)]
pub struct Timeline {
    threshold: u64,
    genesis: Option<u64>,
    last: Option<(u64, u64)>,
    halts: Vec<Halt>,
}

impl Timeline {
    /// A timeline that counts gaps longer than `threshold_seconds` as halts.
    #[must_use]
    pub const fn new(threshold_seconds: u64) -> Self {
        Self {
            threshold: threshold_seconds,
            genesis: None,
            last: None,
            halts: Vec::new(),
        }
    }

    /// The next block, which must be the height after the last one seen
    /// (genesis first). Returns the halt this block ended, if it ended one.
    /// A block out of order is ignored: the caller feeds heights in order.
    ///
    /// The clock starts at block 1. Genesis carries the time its file was
    /// written, which can be long before the network first produced; that
    /// wait is a launch, not a halt.
    pub fn observe(&mut self, height: u64, timestamp: u64) -> Option<Halt> {
        if height == 0 {
            self.last = Some((0, timestamp));
            return None;
        }
        match self.last {
            // The first produced block (or the first seen, when a caller
            // starts past genesis) starts the clock.
            None | Some((0, _)) => {
                self.genesis = Some(timestamp);
                self.last = Some((height, timestamp));
                None
            }
            Some((h, _)) if height != h + 1 => None,
            Some((h, t)) => {
                self.last = Some((height, timestamp));
                // Proposer clocks can disagree by a little; never negative.
                let gap = timestamp.saturating_sub(t);
                (gap > self.threshold).then(|| {
                    let halt = Halt {
                        height: h,
                        started: t,
                        ended: timestamp,
                    };
                    self.halts.push(halt);
                    halt
                })
            }
        }
    }

    /// Newest height seen.
    #[must_use]
    pub fn height(&self) -> Option<u64> {
        self.last.map(|(h, _)| h)
    }

    /// The report at wall-clock time `now` (Unix seconds).
    #[must_use]
    pub fn report(&self, now: u64) -> Option<Report> {
        let genesis = self.genesis?;
        let (height, last_block) = self.last?;
        let since_last_block = now.saturating_sub(last_block);
        let halted_now = since_last_block > self.threshold;
        let elapsed = now.saturating_sub(genesis).max(1);
        let past: u64 = self.halts.iter().map(Halt::seconds).sum();
        let ongoing = if halted_now { since_last_block } else { 0 };
        let down = past.saturating_add(ongoing).min(elapsed);
        #[allow(clippy::cast_precision_loss)]
        let uptime = 1.0 - down as f64 / elapsed as f64;
        let streak_from = self.halts.last().map_or(genesis, |h| h.ended);
        let streak_seconds = if halted_now {
            0
        } else {
            now.saturating_sub(streak_from)
        };
        Some(Report {
            height,
            genesis,
            last_block,
            uptime,
            streak_seconds,
            halted_now,
            since_last_block,
            halts: self.halts.clone(),
            threshold_seconds: self.threshold,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::float_cmp)]
    use super::*;

    fn steady(t: &mut Timeline, from: u64, to: u64, start_ts: u64) -> u64 {
        let mut ts = start_ts;
        for h in from..=to {
            t.observe(h, ts);
            ts += 1;
        }
        ts - 1
    }

    #[test]
    fn a_chain_that_never_stopped_is_fully_up() {
        let mut t = Timeline::new(60);
        let last = steady(&mut t, 0, 1_000, 10_000);
        let r = t.report(last).unwrap();
        assert!(r.halts.is_empty());
        assert_eq!(r.uptime, 1.0);
        // The clock starts at block 1 (timestamp 10_001).
        assert_eq!(r.streak_seconds, 999);
        assert!(!r.halted_now);
    }

    #[test]
    fn a_gap_longer_than_the_threshold_is_a_halt_and_costs_uptime() {
        let mut t = Timeline::new(60);
        steady(&mut t, 0, 99, 0); // heights 0..=99 at 0..=99
        assert_eq!(
            t.observe(100, 99 + 900),
            Some(Halt {
                height: 99,
                started: 99,
                ended: 999
            })
        );
        let last = steady(&mut t, 101, 200, 1_000);
        let r = t.report(last).unwrap();
        assert_eq!(r.halts.len(), 1);
        assert_eq!(r.halts[0].seconds(), 900);
        // Elapsed runs from block 1 (t = 1) to the last block.
        assert!((r.uptime - (1.0 - 900.0 / f64::from(1_098u16))).abs() < 1e-12);
        assert_eq!(r.streak_seconds, last - 999);
    }

    #[test]
    fn a_short_pause_is_not_a_halt() {
        let mut t = Timeline::new(60);
        t.observe(0, 0);
        assert_eq!(
            t.observe(1, 60),
            None,
            "exactly the threshold is not a halt"
        );
        assert_eq!(
            t.observe(2, 121),
            Some(Halt {
                height: 1,
                started: 60,
                ended: 121
            })
        );
    }

    #[test]
    fn a_chain_silent_right_now_is_halted_with_no_streak() {
        let mut t = Timeline::new(60);
        let last = steady(&mut t, 0, 10, 0);
        let r = t.report(last + 300).unwrap();
        assert!(r.halted_now);
        assert_eq!(r.streak_seconds, 0);
        assert_eq!(r.since_last_block, 300);
        assert!(r.uptime < 1.0);
    }

    #[test]
    fn the_wait_between_genesis_and_the_first_block_is_not_a_halt() {
        let mut t = Timeline::new(60);
        t.observe(0, 0);
        assert_eq!(t.observe(1, 5_000), None);
        t.observe(2, 5_001);
        let r = t.report(5_001).unwrap();
        assert!(r.halts.is_empty());
        assert_eq!(r.genesis, 5_000);
        assert_eq!(r.uptime, 1.0);
    }

    #[test]
    fn out_of_order_blocks_change_nothing() {
        let mut t = Timeline::new(60);
        t.observe(0, 0);
        t.observe(1, 1);
        assert_eq!(t.observe(5, 10_000), None);
        assert_eq!(t.height(), Some(1));
    }
}
