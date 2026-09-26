//! Marzullo's algorithm: the interval agreed by the most sources.
//!
//! Each reading is an interval `[offset − u, offset + u]`. Sweep the sorted
//! endpoints, counting how many intervals are open; the region where the
//! count peaks is what the most sources agree on. A source whose interval
//! misses that region is flagged, not averaged in — a single bad GNSS
//! receiver or a spoofed NTP server cannot drag the estimate.

use crate::{Reading, Source};

/// The fused estimate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fused {
    /// Low end of the agreed offset interval, ns.
    pub low_ns: i64,
    /// High end, ns.
    pub high_ns: i64,
    /// Sources that agree.
    pub agreeing: usize,
    /// Sources whose interval misses the agreed region.
    pub outliers: Vec<Source>,
}

impl Fused {
    /// Midpoint offset, ns.
    pub fn offset_ns(&self) -> i64 {
        self.low_ns + (self.high_ns - self.low_ns) / 2
    }

    /// Half-width of the agreed interval: the precision actually achieved.
    pub fn uncertainty_ns(&self) -> u64 {
        self.high_ns.abs_diff(self.low_ns) / 2
    }
}

/// Fuses readings. `None` if no majority agrees on any point (fewer than
/// `readings.len() / 2 + 1` intervals overlap), which a node must treat as
/// "time unknown", never as "use the average".
pub fn marzullo(readings: &[Reading]) -> Option<Fused> {
    let interval = |r: &Reading| {
        let u = i64::try_from(r.uncertainty_ns).unwrap_or(i64::MAX);
        (r.offset_ns.saturating_sub(u), r.offset_ns.saturating_add(u))
    };
    // (point, +1 start / -1 end). Starts sort before ends at the same point
    // so touching intervals count as overlapping.
    let mut edges: Vec<(i64, i8)> = readings
        .iter()
        .flat_map(|r| {
            let (lo, hi) = interval(r);
            [(lo, -1i8), (hi, 1i8)]
        })
        .collect();
    edges.sort_unstable();
    let (mut count, mut best, mut low, mut high) = (0i64, 0i64, 0i64, 0i64);
    for (i, (point, kind)) in edges.iter().enumerate() {
        count -= i64::from(*kind);
        if count > best {
            best = count;
            low = *point;
            high = edges.get(i + 1).map_or(*point, |e| e.0);
        }
    }
    let agreeing = usize::try_from(best).unwrap_or(0);
    if agreeing < readings.len() / 2 + 1 {
        return None;
    }
    let outliers = readings
        .iter()
        .filter(|r| {
            let (lo, hi) = interval(r);
            hi < low || lo > high
        })
        .map(|r| r.source)
        .collect();
    Some(Fused {
        low_ns: low,
        high_ns: high,
        agreeing,
        outliers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(source: Source, offset_ns: i64, uncertainty_ns: u64) -> Reading {
        Reading {
            source,
            offset_ns,
            uncertainty_ns,
        }
    }

    #[test]
    fn a_spoofed_source_is_outvoted_not_averaged() {
        let readings = [
            r(Source::Gnss, 40, 50),
            r(Source::Ptp, 10, 100),
            r(Source::Ntp, 2_000_000, 5_000_000),
            r(Source::Atomic, 30, 20),
            // A spoofed GNSS feed claiming we are 3 ms slow, very confidently.
            r(Source::Pulsar, 3_000_000, 10),
        ];
        let f = marzullo(&readings).expect("majority agrees");
        assert_eq!(f.agreeing, 4);
        assert_eq!(f.outliers, vec![Source::Pulsar]);
        assert!((10..=90).contains(&f.offset_ns()), "{}", f.offset_ns());
        assert!(f.uncertainty_ns() <= 20);
    }

    #[test]
    fn no_majority_means_time_is_unknown() {
        let readings = [
            r(Source::Ntp, 0, 10),
            r(Source::Ptp, 1_000, 10),
            r(Source::Gnss, 2_000, 10),
        ];
        assert_eq!(marzullo(&readings), None);
    }
}
