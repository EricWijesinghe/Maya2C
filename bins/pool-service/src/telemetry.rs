//! Per-worker statistics: what the pool measured, and what the rig claimed.
//!
//! ## Measured and reported are kept apart everywhere
//!
//! Two hash rates appear on every rig. One is derived from accepted share
//! weight and is the pool's own arithmetic; the other arrived in a message the
//! rig composed. Averaging them, or showing whichever is larger, would destroy
//! the only interesting comparison an operator can make — a rig claiming 400
//! MH/s and delivering 40 is a rig with a problem, and the dashboard's job is
//! to make that visible rather than to smooth it away.
//!
//! Power, temperature, and fan speed have no measured counterpart at all. They
//! are carried under `reported_` names and are structurally unable to reach
//! [`crate::pplns`]: nothing in this module is an input to a credit.
//!
//! ## Measuring hash rate from share weight
//!
//! A share worth `2^b` of weight is evidence of about `2^b` hashes. Dividing
//! accumulated weight by elapsed time gives hashes per second directly, with no
//! need to trust anything the rig said.
//!
//! The estimate is smoothed over a sliding window rather than computed per
//! share. Share arrival is Poisson: an instantaneous rate from one interval has
//! a variance as large as its mean, so a per-share figure would swing by
//! multiples between consecutive shares and read as a broken rig.

use std::collections::HashMap;
use std::time::Duration;

use custom_l1_node::state::Address;

use crate::model::{RejectReason, WorkerKey, WorkerStats};

/// Window the hash-rate estimate averages over.
///
/// Ten minutes: long enough that the Poisson variance of share arrival is
/// averaged out at the pool's one-share-per-minute target, short enough that a
/// rig which stopped ten minutes ago is visibly slowing down.
pub const HASHRATE_WINDOW: Duration = Duration::from_secs(600);

/// How long a worker with no shares is still listed.
///
/// A rig that disconnects should not vanish from the dashboard the instant it
/// does — the operator looking for it has just been paged about it. A day is
/// long enough to investigate and short enough that the map does not grow
/// without bound.
pub const WORKER_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// A rig's accumulated statistics.
#[derive(Clone, Debug, Default)]
struct WorkerEntry {
    /// Accepted shares, all time.
    accepted: u64,
    /// Rejected shares, all time.
    rejected: u64,
    /// Stale shares, a subset of `rejected`.
    stale: u64,
    /// Accepted share weight, all time.
    weight: u64,
    /// Share weight inside the current rate window.
    window_weight: u64,
    /// When the current rate window opened, in milliseconds.
    window_start_millis: u64,
    /// Hash rate carried over from the last completed window.
    measured_hashrate: f64,
    /// Most recent accepted share, in milliseconds.
    last_share_millis: Option<u64>,
    /// Last time anything was recorded for this worker.
    last_seen_millis: u64,
    /// Share target currently assigned, in leading zero bits.
    target_bits: u32,
    /// Hash rate the rig claimed.
    reported_hashrate: f32,
    /// Reported wall power, in milliwatts.
    reported_power_milliwatts: u64,
    /// Reported hottest sensor, in millidegrees Celsius.
    reported_temperature_millicelsius: i32,
    /// Reported fan duty cycle.
    reported_fan_percent: u8,
    /// Whether a channel is open for this worker.
    connected: bool,
}

impl WorkerEntry {
    /// Folds the current window into the rate estimate if it has elapsed.
    fn roll_window(&mut self, now_millis: u64) {
        let elapsed = now_millis.saturating_sub(self.window_start_millis);
        let window = HASHRATE_WINDOW.as_millis() as u64;
        if elapsed < window || elapsed == 0 {
            return;
        }

        self.measured_hashrate = self.window_weight as f64 / (elapsed as f64 / 1_000.0);
        self.window_weight = 0;
        self.window_start_millis = now_millis;
    }

    /// The rate to report, blending the closed window with the open one.
    ///
    /// A worker whose first window has not closed yet would otherwise report
    /// zero for its first ten minutes, which is indistinguishable from a rig
    /// that is not working.
    fn hashrate(&self, now_millis: u64) -> f64 {
        let elapsed = now_millis.saturating_sub(self.window_start_millis);
        if elapsed == 0 {
            return self.measured_hashrate;
        }

        let partial = self.window_weight as f64 / (elapsed as f64 / 1_000.0);
        if self.measured_hashrate == 0.0 {
            return partial;
        }
        // Weight the partial window by how much of it has elapsed, so the
        // estimate moves toward the new value rather than jumping to it.
        let fraction = (elapsed as f64 / HASHRATE_WINDOW.as_millis() as f64).min(1.0);
        self.measured_hashrate * (1.0 - fraction) + partial * fraction
    }
}

/// Statistics for every rig the pool has seen.
#[derive(Debug, Default)]
pub struct TelemetryBook {
    /// Entries by worker.
    workers: HashMap<WorkerKey, WorkerEntry>,
}

impl TelemetryBook {
    /// An empty book.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a credited share.
    pub fn record_accepted(&mut self, key: &WorkerKey, weight: u64, now_millis: u64) {
        let entry = self.entry(key, now_millis);
        entry.roll_window(now_millis);
        entry.accepted += 1;
        entry.weight = entry.weight.saturating_add(weight);
        entry.window_weight = entry.window_weight.saturating_add(weight);
        entry.last_share_millis = Some(now_millis);
        entry.last_seen_millis = now_millis;
    }

    /// Records a refused share.
    pub fn record_rejected(&mut self, key: &WorkerKey, reason: RejectReason, now_millis: u64) {
        let entry = self.entry(key, now_millis);
        entry.rejected += 1;
        if reason == RejectReason::Stale {
            // Broken out because staleness is as often the pool's fault as the
            // rig's: slow job push shows up here first.
            entry.stale += 1;
        }
        entry.last_seen_millis = now_millis;
    }

    /// Records the target a channel is now on.
    pub fn record_target(&mut self, key: &WorkerKey, bits: u32, now_millis: u64) {
        let entry = self.entry(key, now_millis);
        entry.target_bits = bits;
        entry.last_seen_millis = now_millis;
    }

    /// Records a channel opening or closing.
    pub fn record_connection(
        &mut self,
        key: &WorkerKey,
        connected: bool,
        reported_hashrate: f32,
        now_millis: u64,
    ) {
        let entry = self.entry(key, now_millis);
        entry.connected = connected;
        if reported_hashrate.is_finite() && reported_hashrate >= 0.0 {
            entry.reported_hashrate = reported_hashrate;
        }
        entry.last_seen_millis = now_millis;
    }

    /// Records a rig's self-reported health.
    ///
    /// None of this is checked, and none of it can be. See the module
    /// documentation and
    /// [`maya_stratum_v2::messages::mining::SubmitWorkerTelemetry`].
    pub fn record_reported_health(
        &mut self,
        key: &WorkerKey,
        power_milliwatts: u64,
        temperature_millicelsius: i32,
        fan_percent: u8,
        now_millis: u64,
    ) {
        let entry = self.entry(key, now_millis);
        entry.reported_power_milliwatts = power_milliwatts;
        entry.reported_temperature_millicelsius = temperature_millicelsius;
        entry.reported_fan_percent = fan_percent;
        entry.last_seen_millis = now_millis;
    }

    /// Renders one worker's statistics.
    #[must_use]
    pub fn stats_for(&self, key: &WorkerKey, now_millis: u64) -> Option<WorkerStats> {
        self.workers
            .get(key)
            .map(|entry| render(key, entry, now_millis))
    }

    /// Every retained worker's statistics.
    #[must_use]
    pub fn snapshot(&self, now_millis: u64) -> Vec<WorkerStats> {
        let mut out: Vec<WorkerStats> = self
            .workers
            .iter()
            .map(|(key, entry)| render(key, entry, now_millis))
            .collect();
        out.sort_by(|a, b| a.miner.cmp(&b.miner).then_with(|| a.worker.cmp(&b.worker)));
        out
    }

    /// Every worker belonging to one miner.
    #[must_use]
    pub fn workers_of(&self, miner: &Address, now_millis: u64) -> Vec<WorkerStats> {
        let mut out: Vec<WorkerStats> = self
            .workers
            .iter()
            .filter(|(key, _)| key.miner == *miner)
            .map(|(key, entry)| render(key, entry, now_millis))
            .collect();
        out.sort_by(|a, b| a.worker.cmp(&b.worker));
        out
    }

    /// The pool's total measured hash rate.
    #[must_use]
    pub fn total_hashrate(&self, now_millis: u64) -> f64 {
        self.workers
            .values()
            .map(|entry| entry.hashrate(now_millis))
            .sum()
    }

    /// Total reported power draw across every connected rig, in milliwatts.
    ///
    /// Reported, and therefore named so. It is a sum of claims.
    #[must_use]
    pub fn total_reported_power_milliwatts(&self) -> u64 {
        self.workers
            .values()
            .filter(|entry| entry.connected)
            .fold(0u64, |sum, entry| {
                sum.saturating_add(entry.reported_power_milliwatts)
            })
    }

    /// Drops workers that have not been seen inside [`WORKER_RETENTION`].
    ///
    /// Returns how many were dropped. Without this the map is unbounded in the
    /// number of rig names that have ever connected.
    pub fn evict_stale(&mut self, now_millis: u64) -> usize {
        let cutoff = WORKER_RETENTION.as_millis() as u64;
        let before = self.workers.len();
        self.workers.retain(|_, entry| {
            entry.connected || now_millis.saturating_sub(entry.last_seen_millis) < cutoff
        });
        before - self.workers.len()
    }

    /// Retained worker count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.workers.len()
    }

    /// Whether any worker is retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }

    /// The entry for a worker, created on first sight.
    fn entry(&mut self, key: &WorkerKey, now_millis: u64) -> &mut WorkerEntry {
        self.workers.entry(key.clone()).or_insert(WorkerEntry {
            window_start_millis: now_millis,
            last_seen_millis: now_millis,
            ..WorkerEntry::default()
        })
    }
}

/// Builds the public view of one entry.
fn render(key: &WorkerKey, entry: &WorkerEntry, now_millis: u64) -> WorkerStats {
    WorkerStats {
        miner: hex::encode(key.miner),
        worker: key.worker.clone(),
        measured_hashrate: entry.hashrate(now_millis),
        reported_hashrate: f64::from(entry.reported_hashrate),
        accepted_shares: entry.accepted,
        rejected_shares: entry.rejected,
        stale_shares: entry.stale,
        accepted_weight: entry.weight,
        target_bits: entry.target_bits,
        reported_power_milliwatts: entry.reported_power_milliwatts,
        reported_temperature_millicelsius: entry.reported_temperature_millicelsius,
        reported_fan_percent: entry.reported_fan_percent,
        seconds_since_share: entry
            .last_share_millis
            .map(|last| now_millis.saturating_sub(last) / 1_000),
        connected: entry.connected,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn key(worker: &str) -> WorkerKey {
        WorkerKey {
            miner: [1u8; 32],
            worker: worker.to_string(),
        }
    }

    #[test]
    fn measured_hash_rate_comes_from_share_weight_alone() {
        // 1,000 shares of 2^10 weight over 100 seconds is 1024 * 1000 / 100
        // hashes per second, and nothing the rig said enters the calculation.
        let mut book = TelemetryBook::new();
        let worker = key("rig-1");
        book.record_connection(&worker, true, 999_999.0, 0);

        for index in 0..1_000u64 {
            book.record_accepted(&worker, 1 << 10, index * 100);
        }

        let stats = book.stats_for(&worker, 100_000).unwrap();
        let expected = (1_000.0 * 1024.0) / 100.0;
        assert!(
            (stats.measured_hashrate - expected).abs() < expected * 0.01,
            "measured {} against {expected}",
            stats.measured_hashrate
        );
    }

    #[test]
    fn a_reported_hash_rate_never_touches_the_measured_one() {
        let mut book = TelemetryBook::new();
        let worker = key("rig-1");
        book.record_connection(&worker, true, 400_000_000.0, 0);
        book.record_accepted(&worker, 1 << 10, 1_000);

        let stats = book.stats_for(&worker, 2_000).unwrap();
        assert_eq!(stats.reported_hashrate, 400_000_000.0);
        assert!(
            stats.measured_hashrate < 100_000.0,
            "the claim leaked into the measurement: {}",
            stats.measured_hashrate
        );
    }

    #[test]
    fn stale_shares_are_counted_inside_rejections_and_separately() {
        let mut book = TelemetryBook::new();
        let worker = key("rig-1");

        book.record_rejected(&worker, RejectReason::Stale, 0);
        book.record_rejected(&worker, RejectReason::LowDifficulty, 1);
        book.record_accepted(&worker, 1, 2);

        let stats = book.stats_for(&worker, 3).unwrap();
        assert_eq!(stats.rejected_shares, 2);
        assert_eq!(stats.stale_shares, 1);
        assert_eq!(stats.efficiency(), Some(1.0 / 3.0));
    }

    #[test]
    fn reported_health_is_stored_verbatim_including_sub_zero_temperatures() {
        let mut book = TelemetryBook::new();
        let worker = key("rig-1");
        book.record_reported_health(&worker, 1_450_000, -18_000, 80, 0);

        let stats = book.stats_for(&worker, 0).unwrap();
        assert_eq!(stats.reported_power_milliwatts, 1_450_000);
        assert_eq!(stats.reported_temperature_millicelsius, -18_000);
        assert_eq!(stats.reported_fan_percent, 80);
    }

    #[test]
    fn joules_per_hash_needs_both_halves() {
        let mut book = TelemetryBook::new();
        let worker = key("rig-1");
        book.record_reported_health(&worker, 1_000_000, 40_000, 50, 0);

        // No shares yet, so there is no measured rate to divide by. Reporting a
        // number here would be reporting one made entirely of claims.
        let stats = book.stats_for(&worker, 0).unwrap();
        assert_eq!(stats.reported_joules_per_hash(), None);
    }

    #[test]
    fn a_disconnected_worker_is_retained_then_evicted() {
        let mut book = TelemetryBook::new();
        let worker = key("rig-1");
        book.record_connection(&worker, true, 1.0, 0);
        book.record_connection(&worker, false, 1.0, 1_000);

        let inside = WORKER_RETENTION.as_millis() as u64;
        assert_eq!(book.evict_stale(inside), 0, "still investigable");
        assert_eq!(book.evict_stale(inside + 2_000), 1);
        assert!(book.is_empty());
    }

    #[test]
    fn a_connected_worker_is_never_evicted() {
        let mut book = TelemetryBook::new();
        let worker = key("rig-1");
        book.record_connection(&worker, true, 1.0, 0);

        assert_eq!(
            book.evict_stale(WORKER_RETENTION.as_millis() as u64 * 10),
            0
        );
    }

    #[test]
    fn a_non_finite_reported_hash_rate_is_ignored_rather_than_stored() {
        // The codec refuses these on the wire; this is the second line, for a
        // value that reached the book by some other route.
        let mut book = TelemetryBook::new();
        let worker = key("rig-1");
        book.record_connection(&worker, true, 100.0, 0);
        book.record_connection(&worker, true, f32::NAN, 1);

        assert_eq!(book.stats_for(&worker, 1).unwrap().reported_hashrate, 100.0);
    }

    #[test]
    fn workers_are_grouped_under_their_miner() {
        let mut book = TelemetryBook::new();
        book.record_connection(&key("rig-1"), true, 1.0, 0);
        book.record_connection(&key("rig-2"), true, 1.0, 0);
        book.record_connection(
            &WorkerKey {
                miner: [2u8; 32],
                worker: "other".to_string(),
            },
            true,
            1.0,
            0,
        );

        assert_eq!(book.workers_of(&[1u8; 32], 0).len(), 2);
        assert_eq!(book.workers_of(&[2u8; 32], 0).len(), 1);
        assert_eq!(book.snapshot(0).len(), 3);
    }

    #[test]
    fn total_reported_power_counts_only_connected_rigs() {
        let mut book = TelemetryBook::new();
        let live = key("rig-1");
        let gone = key("rig-2");

        book.record_connection(&live, true, 1.0, 0);
        book.record_reported_health(&live, 1_000_000, 0, 50, 0);
        book.record_connection(&gone, false, 1.0, 0);
        book.record_reported_health(&gone, 9_000_000, 0, 50, 0);

        assert_eq!(book.total_reported_power_milliwatts(), 1_000_000);
    }
}
