//! Timing service (Master Prompt 7 §5).
//!
//! # What precision is physically on offer
//!
//! | Source | Typical accuracy to UTC | Why |
//! |---|---|---|
//! | NTP over the internet | 1-50 ms | asymmetric, variable paths |
//! | PTP (IEEE 1588) with hardware timestamps | 10 ns - 1 µs | on a controlled LAN |
//! | GNSS receiver (GPS/Galileo) | 20-50 ns | after the receiver's own corrections |
//! | Caesium / hydrogen-maser reference | sub-ns holdover, drifts over days | local only |
//!
//! Sub-picosecond agreement *across a network* is not achievable: a
//! picosecond is 0.3 mm of light travel, and path-length uncertainty on any
//! real link is orders of magnitude larger. This crate therefore aims at
//! **bounded** agreement — an interval every honest source agrees on — and the
//! chain's rules only ever ask "is this timestamp within `D` of my clock",
//! never "what time is it exactly" ([`within_drift`]).
//!
//! # Modules
//!
//! - [`fuse`]: Marzullo's algorithm over source intervals: the smallest
//!   interval consistent with the most sources, so a lying or broken source
//!   is outvoted rather than averaged in.
//! - [`relativity`]: special + general relativistic rate offsets for orbital
//!   nodes (GPS: −7.2 µs/day SR, +45.7 µs/day GR). Floats; off-consensus.
//! - Pulsar timing is a SIM input: [`Source::Pulsar`], an interval like any
//!   other, with the microsecond-scale uncertainty millisecond pulsars give
//!   after a long integration.

#![warn(missing_docs)]

pub mod fuse;
pub mod relativity;

/// A time source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// Network Time Protocol.
    Ntp,
    /// Precision Time Protocol.
    Ptp,
    /// GPS or Galileo receiver.
    Gnss,
    /// Local atomic reference.
    Atomic,
    /// Millisecond-pulsar timing. SIM input.
    Pulsar,
}

/// One source's reading: its offset from the local clock and its stated
/// uncertainty, both in nanoseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    /// Which source.
    pub source: Source,
    /// Estimated offset of true time from the local clock, ns.
    pub offset_ns: i64,
    /// Half-width of the source's confidence interval, ns.
    pub uncertainty_ns: u64,
}

/// The consensus-safe timestamp check: whether `timestamp_ms` is no more
/// than `max_drift_ms` away from `local_ms`, in either direction. Integer-only
/// and symmetric; this is the only time rule a consensus path may use.
pub const fn within_drift(timestamp_ms: u64, local_ms: u64, max_drift_ms: u64) -> bool {
    timestamp_ms.abs_diff(local_ms) <= max_drift_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drift_check_is_symmetric_and_inclusive() {
        assert!(within_drift(1_000, 1_500, 500));
        assert!(within_drift(2_000, 1_500, 500));
        assert!(!within_drift(2_001, 1_500, 500));
        assert!(!within_drift(0, u64::MAX, 1));
    }
}
