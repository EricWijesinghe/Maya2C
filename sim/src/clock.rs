//! Virtual time.
//!
//! Nothing in a simulation may read a wall clock. A test that sleeps is a
//! test whose outcome depends on how loaded the machine was, and a chaos test
//! that takes real seconds per simulated second cannot run a day of network
//! partitions in CI.
//!
//! Time here is an integer count of nanoseconds that only moves when the
//! scheduler moves it, so a simulated hour costs whatever the work costs and
//! nothing for the waiting.

use core::fmt;

/// A point in virtual time, in nanoseconds since the start of the run.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Instant(u64);

/// A span of virtual time, in nanoseconds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Duration(u64);

impl Duration {
    /// A span of `n` nanoseconds.
    #[must_use]
    pub const fn from_nanos(n: u64) -> Self {
        Self(n)
    }
    /// A span of `n` microseconds.
    #[must_use]
    pub const fn from_micros(n: u64) -> Self {
        Self(n.saturating_mul(1_000))
    }
    /// A span of `n` milliseconds.
    #[must_use]
    pub const fn from_millis(n: u64) -> Self {
        Self(n.saturating_mul(1_000_000))
    }
    /// A span of `n` seconds.
    #[must_use]
    pub const fn from_secs(n: u64) -> Self {
        Self(n.saturating_mul(1_000_000_000))
    }
    /// The span in nanoseconds.
    #[must_use]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
    /// The span in whole milliseconds.
    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0 / 1_000_000
    }

    /// Zero.
    pub const ZERO: Self = Self(0);
}

impl Instant {
    /// The start of the run.
    pub const START: Self = Self(0);

    /// This instant advanced by `d`. Saturating, so a simulation cannot wrap
    /// time into the past by scheduling far enough ahead.
    #[must_use]
    pub const fn saturating_add(self, d: Duration) -> Self {
        Self(self.0.saturating_add(d.0))
    }

    /// How long since `earlier`, or zero if `earlier` is later.
    #[must_use]
    pub const fn since(self, earlier: Self) -> Duration {
        Duration(self.0.saturating_sub(earlier.0))
    }

    /// Nanoseconds since the start of the run.
    #[must_use]
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
}

impl core::ops::Add<Duration> for Instant {
    type Output = Self;
    fn add(self, d: Duration) -> Self {
        self.saturating_add(d)
    }
}

impl core::ops::Add for Duration {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl fmt::Debug for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t+{}", Duration(self.0))
    }
}

impl fmt::Display for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t+{}", Duration(self.0))
    }
}

impl fmt::Debug for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.0;
        if n >= 1_000_000_000 {
            write!(
                f,
                "{}.{:03}s",
                n / 1_000_000_000,
                (n % 1_000_000_000) / 1_000_000
            )
        } else if n >= 1_000_000 {
            write!(f, "{}.{:03}ms", n / 1_000_000, (n % 1_000_000) / 1_000)
        } else if n >= 1_000 {
            write!(f, "{}.{:03}us", n / 1_000, n % 1_000)
        } else {
            write!(f, "{n}ns")
        }
    }
}

/// The simulation's clock. It moves only when told to.
#[derive(Clone, Debug, Default)]
pub struct Clock {
    now: Instant,
}

impl Clock {
    /// A clock at the start of the run.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            now: Instant::START,
        }
    }

    /// The current virtual time.
    #[must_use]
    pub const fn now(&self) -> Instant {
        self.now
    }

    /// Move to `to`. Time never runs backwards: an event scheduled in the
    /// past is delivered now rather than rewinding the clock, because a
    /// rewind would let one late event un-happen an earlier one.
    pub fn advance_to(&mut self, to: Instant) {
        if to > self.now {
            self.now = to;
        }
    }

    /// Move forward by `d`.
    pub fn advance(&mut self, d: Duration) {
        self.now = self.now.saturating_add(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_moves_only_forward() {
        let mut clock = Clock::new();
        clock.advance(Duration::from_millis(100));
        let later = clock.now();
        clock.advance_to(Instant::START);
        assert_eq!(clock.now(), later, "the clock rewound");
    }

    #[test]
    fn durations_render_at_a_readable_scale() {
        assert_eq!(Duration::from_nanos(500).to_string(), "500ns");
        assert_eq!(Duration::from_micros(1).to_string(), "1.000us");
        assert_eq!(Duration::from_millis(250).to_string(), "250.000ms");
        assert_eq!(Duration::from_secs(3).to_string(), "3.000s");
    }

    #[test]
    fn arithmetic_saturates_instead_of_wrapping_into_the_past() {
        let far = Instant::START.saturating_add(Duration::from_nanos(u64::MAX));
        assert_eq!(far.saturating_add(Duration::from_secs(1)), far);
        assert_eq!(Instant::START.since(far), Duration::ZERO);
    }

    #[test]
    fn since_measures_the_gap() {
        let a = Instant::START.saturating_add(Duration::from_millis(10));
        let b = a.saturating_add(Duration::from_millis(15));
        assert_eq!(b.since(a), Duration::from_millis(15));
    }
}
