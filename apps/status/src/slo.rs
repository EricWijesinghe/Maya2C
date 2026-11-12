//! Service levels from real data only. A figure that was not measured is
//! `None`, which the API sends as `null` and the page shows as MISSING.
//!
//! Uptime is recomputed from block timestamps over a 30-day window, the same
//! halts the status page shows. Finality comes from the node's own Prometheus
//! histogram (`maya_finality_latency_seconds`), read as a scrape: quantiles
//! are interpolated the way Prometheus' `histogram_quantile` does, and they
//! cover the time since the node started, not a rolling window.
//!
//! Targets are the design goals in `docs/SLO.md`, not measurements.

use serde::Serialize;

use crate::Report;

/// The SLO window, seconds.
pub const WINDOW_SECS: u64 = 30 * 86_400;
/// Uptime target, percent (`docs/SLO.md` liveness row).
pub const UPTIME_TARGET_PCT: f64 = 99.9;
/// Finality median target, seconds (`docs/SLO.md` finality-p50).
pub const FINALITY_P50_TARGET_S: f64 = 2.0;
/// Finality 99th percentile target, seconds (`docs/SLO.md` finality-p99).
pub const FINALITY_P99_TARGET_S: f64 = 6.0;
/// The node's finality histogram, as exported.
pub const FINALITY_METRIC: &str = "maya_finality_latency_seconds";

/// A measured finality distribution.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Finality {
    /// Blocks observed by the node since it started.
    pub samples: u64,
    /// Median, seconds.
    pub p50_s: f64,
    /// 99th percentile, seconds.
    pub p99_s: f64,
    /// When the scrape was read, Unix seconds.
    pub measured_at: u64,
}

/// Cumulative `(upper bound, count)` buckets, ascending, ending at `+Inf`.
type Buckets = Vec<(f64, u64)>;

/// Reads the histogram called `metric` out of a Prometheus text exposition.
#[must_use]
pub fn histogram(text: &str, metric: &str) -> Option<Buckets> {
    let bucket_prefix = format!("{metric}_bucket{{le=\"");
    let mut buckets: Buckets = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix(&bucket_prefix) else {
            continue;
        };
        let (le, tail) = rest.split_once("\"}")?;
        let bound = if le == "+Inf" {
            f64::INFINITY
        } else {
            le.parse().ok()?
        };
        let count = tail.trim().parse::<f64>().ok()?;
        // The exposition writes whole counts; a fraction means it is not one.
        if count < 0.0 || count.fract() != 0.0 {
            return None;
        }
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        buckets.push((bound, count as u64));
    }
    (!buckets.is_empty() && buckets.last()?.0.is_infinite()).then_some(buckets)
}

/// The `q`-quantile by linear interpolation within the bucket that holds it,
/// as Prometheus' `histogram_quantile`. `None` with no samples; the largest
/// finite bound when the quantile lies in the `+Inf` bucket.
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub fn quantile(buckets: &[(f64, u64)], q: f64) -> Option<f64> {
    let total = buckets.last()?.1;
    if total == 0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    let rank = q * total as f64;
    let mut lower = (0.0_f64, 0_u64);
    for &(bound, cum) in buckets {
        if (cum as f64) >= rank {
            if bound.is_infinite() {
                return Some(lower.0);
            }
            #[allow(clippy::cast_precision_loss)]
            let inside = (cum - lower.1) as f64;
            if inside == 0.0 {
                return Some(bound);
            }
            #[allow(clippy::cast_precision_loss)]
            let frac = (rank - lower.1 as f64) / inside;
            return Some(lower.0 + (bound - lower.0) * frac);
        }
        lower = (bound, cum);
    }
    None
}

/// The finality figures in a node's exposition, or `None` when the node has
/// observed no blocks (not measured, rather than zero).
#[must_use]
pub fn finality_from(text: &str, measured_at: u64) -> Option<Finality> {
    let buckets = histogram(text, FINALITY_METRIC)?;
    let samples = buckets.last()?.1;
    Some(Finality {
        samples,
        p50_s: quantile(&buckets, 0.50)?,
        p99_s: quantile(&buckets, 0.99)?,
        measured_at,
    })
}

/// Percent of the last [`WINDOW_SECS`] the chain was producing, from the
/// report's halts. The window starts at genesis for a younger chain. `None`
/// when the window is empty.
#[must_use]
pub fn uptime_window(report: &Report, now: u64) -> Option<f64> {
    let start = report.genesis.max(now.saturating_sub(WINDOW_SECS));
    let elapsed = now.checked_sub(start).filter(|e| *e > 0)?;
    let past: u64 = report
        .halts
        .iter()
        .map(|h| h.ended.min(now).saturating_sub(h.started.max(start)))
        .sum();
    let ongoing = if report.halted_now {
        now.saturating_sub(report.last_block.max(start))
    } else {
        0
    };
    let down = past.saturating_add(ongoing).min(elapsed);
    #[allow(clippy::cast_precision_loss)]
    let pct = 100.0 * (1.0 - down as f64 / elapsed as f64);
    Some(pct)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::Halt;

    const EXPOSITION: &str = "\
# TYPE maya_finality_latency_seconds histogram
maya_finality_latency_seconds_bucket{le=\"0.5\"} 10
maya_finality_latency_seconds_bucket{le=\"1\"} 50
maya_finality_latency_seconds_bucket{le=\"2\"} 90
maya_finality_latency_seconds_bucket{le=\"6\"} 100
maya_finality_latency_seconds_bucket{le=\"+Inf\"} 100
maya_finality_latency_seconds_sum 150
maya_finality_latency_seconds_count 100
";

    #[test]
    fn quantiles_interpolate_inside_the_bucket_that_holds_them() {
        let b = histogram(EXPOSITION, FINALITY_METRIC).unwrap();
        // Rank 50 lands exactly on the 1 s bound.
        assert!((quantile(&b, 0.50).unwrap() - 1.0).abs() < 1e-9);
        // Rank 99 sits 9/10 of the way from 2 s to 6 s.
        assert!((quantile(&b, 0.99).unwrap() - 5.6).abs() < 1e-9);
    }

    #[test]
    fn no_samples_is_missing_not_zero() {
        let text = EXPOSITION
            .replace(" 100\n", " 0\n")
            .replace("} 10\n", "} 0\n");
        assert!(finality_from(&text, 1).is_none());
        assert!(finality_from("", 1).is_none(), "no histogram at all");
    }

    #[test]
    fn a_tail_bucket_answers_with_the_largest_finite_bound() {
        let b = [(1.0, 1), (f64::INFINITY, 10)];
        assert_eq!(quantile(&b, 0.99), Some(1.0));
    }

    #[test]
    fn a_fractional_or_missing_inf_bucket_is_refused() {
        assert!(
            histogram(
                "maya_finality_latency_seconds_bucket{le=\"1\"} 1.5\n",
                FINALITY_METRIC
            )
            .is_none()
        );
        assert!(
            histogram(
                "maya_finality_latency_seconds_bucket{le=\"1\"} 1\n",
                FINALITY_METRIC
            )
            .is_none()
        );
    }

    fn report(halts: Vec<Halt>, halted_now: bool, last_block: u64, genesis: u64) -> Report {
        Report {
            height: 1,
            genesis,
            last_block,
            uptime: 1.0,
            streak_seconds: 0,
            halted_now,
            since_last_block: 0,
            halts,
            threshold_seconds: 60,
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn uptime_counts_only_halts_inside_the_window() {
        let now = 100 * 86_400;
        let old = Halt {
            height: 1,
            started: 10,
            ended: 10 + 86_400,
        }; // outside the 30 days
        let recent = Halt {
            height: 2,
            started: now - 864,
            ended: now - 864 + 86,
        }; // 86 s
        let r = report(vec![old, recent], false, now, 0);
        let pct = uptime_window(&r, now).unwrap();
        let expected = 100.0 * (1.0 - 86.0 / WINDOW_SECS as f64);
        assert!((pct - expected).abs() < 1e-6, "{pct} vs {expected}");
    }

    #[test]
    fn an_ongoing_halt_counts_to_now_and_a_young_chain_uses_its_own_genesis() {
        let now = 1_000;
        let r = report(vec![], true, 900, 500);
        // Window starts at genesis (500): 500 s elapsed, 100 s halted.
        let pct = uptime_window(&r, now).unwrap();
        assert!((pct - 80.0).abs() < 1e-9, "{pct}");
        assert!(uptime_window(&report(vec![], false, 0, 1_000), 1_000).is_none());
    }
}
