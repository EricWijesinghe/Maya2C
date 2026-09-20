//! Reports in, one snapshot out.
//!
//! # Height is a median, not a maximum
//!
//! The obvious way to report chain height across a network is to take the
//! largest number anybody claims. It is also the wrong way: the maximum is
//! whatever the most-wrong reporter says, so a single node with a corrupted
//! database or a made-up number sets the network's headline height and it
//! never comes back down. The median moves only when most reporters move,
//! which is the property a headline number needs.
//!
//! The same applies to propagation, where the input is worse still — a
//! difference between two unsynchronised clocks, one of which a miner writes
//! freely. A median across reporters turns one node's skew into an outlier
//! instead of into the number.
//!
//! Hash rate is the exception, and is summed. There is no median to take: the
//! network's rate is the total of what is running, and a reporter who does not
//! report contributes nothing rather than pulling an average down.
//!
//! # Everything is bounded by the TTL, not by the reporter
//!
//! A reporter that stops reporting disappears from the next snapshot. That is
//! the only way a total goes down, because reporters crash rather than saying
//! goodbye.

use std::collections::HashMap;

use crate::region::{self, Region};
use crate::report::{Report, ReportError, ReporterId, Stored};
use crate::snapshot::{BackendSummary, RegionSummary, Snapshot};

/// Reporters tracked before the oldest are dropped.
///
/// The map is the memory an attacker grows by varying their reporter id, so
/// it needs a ceiling. Expired entries go first; if every entry is live, the
/// oldest is evicted — a collector that stopped accepting new reporters
/// entirely would let one flood freeze the dashboard at whoever arrived
/// first.
pub const MAX_REPORTERS: usize = 100_000;

/// Live reports, keyed by reporter.
#[derive(Debug, Default)]
pub struct Collector {
    reports: HashMap<ReporterId, Stored>,
}

impl Collector {
    /// A collector with no reports.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Accepts a report, replacing any previous one from the same reporter.
    ///
    /// Replacing rather than accumulating is what stops a reporter inflating
    /// the network total by sending the same figure a thousand times.
    ///
    /// # Errors
    ///
    /// The [`ReportError`] from [`Report::validate`].
    pub fn accept(&mut self, report: Report, region: Region, now: u64) -> Result<(), ReportError> {
        report.validate()?;

        if self.reports.len() >= MAX_REPORTERS && !self.reports.contains_key(report.reporter()) {
            self.evict(now);
        }

        self.reports.insert(
            report.reporter().clone(),
            Stored {
                report,
                region,
                received_at: now,
            },
        );
        Ok(())
    }

    /// Drops reports older than the TTL.
    pub fn sweep(&mut self, now: u64) {
        self.reports.retain(|_, stored| stored.is_live(now));
    }

    /// Reporters currently held, live or not.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.reports.len()
    }

    /// Builds the snapshot the dashboard renders.
    #[must_use]
    pub fn snapshot(&self, now: u64) -> Snapshot {
        let live: Vec<&Stored> = self
            .reports
            .values()
            .filter(|stored| stored.is_live(now))
            .collect();

        let mut hash_rate = 0u64;
        let mut heights = Vec::new();
        let mut peers = Vec::new();
        let mut propagation = Vec::new();
        let mut miners = 0usize;
        let mut nodes = 0usize;

        // (reporters, hash rate) per country, before folding.
        let mut by_region: HashMap<Region, (usize, u64)> = HashMap::new();
        let mut by_backend: HashMap<&str, (usize, u64)> = HashMap::new();

        for stored in &live {
            let entry = by_region.entry(stored.region).or_insert((0, 0));
            entry.0 += 1;

            match &stored.report {
                Report::Miner(report) => {
                    miners += 1;
                    // Saturating rather than checked: every report is already
                    // bounded by `MAX_HASH_RATE`, so reaching `u64::MAX` needs
                    // more reporters than `MAX_REPORTERS` allows. Saturating
                    // is the safe reading if that ever stops being true.
                    hash_rate = hash_rate.saturating_add(report.hash_rate);
                    entry.1 = entry.1.saturating_add(report.hash_rate);

                    let backend = by_backend.entry(report.backend.as_str()).or_insert((0, 0));
                    backend.0 += 1;
                    backend.1 = backend.1.saturating_add(report.hash_rate);
                }
                Report::Node(report) => {
                    nodes += 1;
                    heights.push(report.height);
                    peers.push(report.peers);
                    if let Some(ms) = report.propagation_ms {
                        propagation.push(ms);
                    }
                }
            }
        }

        let mut regions: Vec<RegionSummary> =
            region::fold_small(by_region, |total: &mut u64, value| {
                *total = total.saturating_add(value);
            })
            .into_iter()
            .map(|(region, (reporters, hash_rate))| RegionSummary {
                region,
                reporters,
                hash_rate,
            })
            .collect();
        // Sorted by hash rate, then by code, so a snapshot with unchanged
        // contents is byte-identical between calls. A dashboard whose rows
        // reshuffle on every poll is unreadable, and a test that compared two
        // snapshots would be flaky for no reason.
        regions.sort_by(|a, b| {
            b.hash_rate
                .cmp(&a.hash_rate)
                .then_with(|| a.region.cmp(&b.region))
        });

        let mut backends: Vec<BackendSummary> = by_backend
            .into_iter()
            .map(|(backend, (miners, hash_rate))| BackendSummary {
                backend: backend.to_string(),
                miners,
                hash_rate,
            })
            .collect();
        backends.sort_by(|a, b| {
            b.hash_rate
                .cmp(&a.hash_rate)
                .then_with(|| a.backend.cmp(&b.backend))
        });

        Snapshot {
            reporters: live.len(),
            miners,
            nodes,
            hash_rate,
            height: median(&mut heights),
            peers: median(&mut peers),
            propagation_ms: median(&mut propagation),
            regions,
            backends,
            taken_at: now,
        }
    }

    /// Makes room by dropping expired reports, or the oldest if none expired.
    fn evict(&mut self, now: u64) {
        let before = self.reports.len();
        self.sweep(now);
        if self.reports.len() < before {
            return;
        }

        // Every report is live. Drop the oldest rather than refusing the new
        // one: refusing would let whoever arrived first hold the dashboard
        // frozen against every reporter that came after.
        if let Some(oldest) = self
            .reports
            .iter()
            .min_by_key(|(id, stored)| (stored.received_at, (*id).clone()))
            .map(|(id, _)| id.clone())
        {
            self.reports.remove(&oldest);
        }
    }
}

/// The median of `values`, or `None` when there are none.
///
/// The lower of the two middle values on an even count, so the result is
/// always a figure somebody actually reported. Interpolating between two
/// heights would produce a height no node has.
fn median<T: Copy + Ord>(values: &mut [T]) -> Option<T> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[(values.len() - 1) / 2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{MinerReport, NodeReport};

    fn id(name: &str) -> ReporterId {
        ReporterId::parse(name).expect("a valid id")
    }

    fn miner(name: &str, hash_rate: u64, backend: &str) -> Report {
        Report::Miner(MinerReport {
            reporter: id(name),
            hash_rate,
            backend: backend.to_string(),
            device: "test device".to_string(),
        })
    }

    fn node(name: &str, height: u64, peers: u32, propagation_ms: Option<u64>) -> Report {
        Report::Node(NodeReport {
            reporter: id(name),
            height,
            peers,
            propagation_ms,
            client: "maya/0.1.0".to_string(),
        })
    }

    #[test]
    fn an_empty_collector_reports_nothing_rather_than_zero_height() {
        // `Some(0)` would be a claim that the chain is at genesis. `None` is
        // the truth: nobody is reporting.
        let snapshot = Collector::new().snapshot(1_000);
        assert_eq!(snapshot.reporters, 0);
        assert_eq!(snapshot.height, None);
        assert_eq!(snapshot.hash_rate, 0);
    }

    #[test]
    fn hash_rate_is_summed_across_miners() {
        let mut collector = Collector::new();
        for (index, rate) in [100u64, 200, 300].into_iter().enumerate() {
            collector
                .accept(
                    miner(&format!("rig-{index}"), rate, "wgpu"),
                    Region::parse("US"),
                    1_000,
                )
                .expect("valid");
        }

        let snapshot = collector.snapshot(1_000);
        assert_eq!(snapshot.hash_rate, 600);
        assert_eq!(snapshot.miners, 3);
        assert_eq!(snapshot.nodes, 0);
    }

    #[test]
    fn one_reporter_cannot_inflate_the_total_by_repeating_itself() {
        // Reports replace rather than accumulate. Without this, a loop is a
        // hash-rate multiplier.
        let mut collector = Collector::new();
        for _ in 0..1_000 {
            collector
                .accept(miner("rig-1", 500, "wgpu"), Region::parse("US"), 1_000)
                .expect("valid");
        }

        let snapshot = collector.snapshot(1_000);
        assert_eq!(snapshot.hash_rate, 500);
        assert_eq!(snapshot.miners, 1);
    }

    #[test]
    fn height_is_a_median_so_one_wrong_node_cannot_set_it() {
        // The property the maximum does not have. A node claiming height
        // 9,000,000 would otherwise become the network's headline height and
        // never come back down.
        let mut collector = Collector::new();
        collector
            .accept(node("a", 1_000, 8, None), Region::parse("US"), 1_000)
            .expect("valid");
        collector
            .accept(node("b", 1_001, 8, None), Region::parse("DE"), 1_000)
            .expect("valid");
        collector
            .accept(node("liar", 9_000_000, 8, None), Region::parse("FR"), 1_000)
            .expect("valid");

        assert_eq!(collector.snapshot(1_000).height, Some(1_001));
    }

    #[test]
    fn propagation_is_a_median_because_its_input_is_two_unsynced_clocks() {
        // A miner writes `header.timestamp` freely — the chain has no
        // future-drift bound. One reporter's skew must be an outlier, not the
        // number.
        let mut collector = Collector::new();
        for (index, ms) in [800u64, 900, 1_000, 50_000].into_iter().enumerate() {
            collector
                .accept(
                    node(&format!("n{index}"), 100, 8, Some(ms)),
                    Region::parse("US"),
                    1_000,
                )
                .expect("valid");
        }

        assert_eq!(collector.snapshot(1_000).propagation_ms, Some(900));
    }

    #[test]
    fn a_node_without_a_propagation_figure_does_not_become_a_zero() {
        // `None` must not be counted as "instant". A node that has not
        // imported a block yet has no figure, and averaging in a zero would
        // make the network look faster than it is.
        let mut collector = Collector::new();
        collector
            .accept(node("a", 100, 8, Some(1_000)), Region::parse("US"), 1_000)
            .expect("valid");
        collector
            .accept(node("b", 100, 8, None), Region::parse("US"), 1_000)
            .expect("valid");

        assert_eq!(collector.snapshot(1_000).propagation_ms, Some(1_000));
    }

    #[test]
    fn a_reporter_that_stops_reporting_leaves_the_total() {
        // The only way a total goes down, and the reason the TTL exists.
        let mut collector = Collector::new();
        collector
            .accept(miner("rig-1", 500, "wgpu"), Region::parse("US"), 1_000)
            .expect("valid");

        assert_eq!(collector.snapshot(1_000).hash_rate, 500);
        assert_eq!(
            collector
                .snapshot(1_000 + crate::report::REPORT_TTL_SECONDS)
                .hash_rate,
            0
        );
    }

    #[test]
    fn small_countries_are_folded_in_the_snapshot() {
        // The privacy floor, applied where the dashboard reads it rather than
        // at ingest — a country can cross the threshold in both directions.
        let mut collector = Collector::new();
        for index in 0..6 {
            collector
                .accept(
                    miner(&format!("us-{index}"), 100, "wgpu"),
                    Region::parse("US"),
                    1_000,
                )
                .expect("valid");
        }
        collector
            .accept(miner("li-0", 50, "cpu"), Region::parse("LI"), 1_000)
            .expect("valid");

        let snapshot = collector.snapshot(1_000);
        let named: Vec<&Region> = snapshot.regions.iter().map(|r| &r.region).collect();
        assert!(named.contains(&&Region::parse("US")));
        assert!(!named.contains(&&Region::parse("LI")));
        assert!(named.contains(&&Region::OTHER));

        let total: u64 = snapshot.regions.iter().map(|r| r.hash_rate).sum();
        assert_eq!(total, snapshot.hash_rate, "folding must conserve the total");
    }

    #[test]
    fn backends_are_broken_out_so_the_gpu_share_is_visible() {
        let mut collector = Collector::new();
        collector
            .accept(miner("gpu-1", 1_000, "wgpu"), Region::parse("US"), 1_000)
            .expect("valid");
        collector
            .accept(miner("gpu-2", 2_000, "cuda"), Region::parse("US"), 1_000)
            .expect("valid");
        collector
            .accept(miner("cpu-1", 10, "cpu"), Region::parse("US"), 1_000)
            .expect("valid");

        let snapshot = collector.snapshot(1_000);
        assert_eq!(snapshot.backends.len(), 3);
        // Sorted by hash rate descending, so the chart reads top-down.
        assert_eq!(snapshot.backends[0].backend, "cuda");
        assert_eq!(snapshot.backends[0].hash_rate, 2_000);
        assert_eq!(snapshot.backends[2].backend, "cpu");
    }

    #[test]
    fn two_snapshots_of_unchanged_state_are_identical() {
        // A dashboard whose rows reshuffle on every poll is unreadable, and
        // `HashMap` iteration order is not stable between calls.
        let mut collector = Collector::new();
        for index in 0..20 {
            collector
                .accept(
                    miner(
                        &format!("rig-{index}"),
                        100,
                        if index % 2 == 0 { "cuda" } else { "wgpu" },
                    ),
                    Region::parse(if index % 3 == 0 { "US" } else { "DE" }),
                    1_000,
                )
                .expect("valid");
        }

        assert_eq!(collector.snapshot(1_000), collector.snapshot(1_000));
    }

    #[test]
    fn an_invalid_report_is_refused_and_stores_nothing() {
        let mut collector = Collector::new();
        assert!(
            collector
                .accept(miner("rig-1", u64::MAX, "wgpu"), Region::UNKNOWN, 1_000)
                .is_err()
        );
        assert_eq!(collector.tracked(), 0);
    }

    #[test]
    fn a_new_reporter_is_never_locked_out_by_a_full_table() {
        // Eviction drops the oldest rather than refusing the newest. The
        // alternative lets whoever floods first freeze the dashboard.
        let mut collector = Collector::new();
        for index in 0..MAX_REPORTERS {
            collector
                .accept(
                    miner(&format!("r{index}"), 1, "cpu"),
                    Region::UNKNOWN,
                    1_000,
                )
                .expect("valid");
        }
        assert_eq!(collector.tracked(), MAX_REPORTERS);

        collector
            .accept(miner("newcomer", 1, "cpu"), Region::UNKNOWN, 1_000)
            .expect("valid");
        assert!(collector.tracked() <= MAX_REPORTERS);

        let snapshot = collector.snapshot(1_000);
        assert!(snapshot.reporters > 0);
    }
}
