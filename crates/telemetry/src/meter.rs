//! Turning a hash counter into a hash rate.
//!
//! # This is the miner-side half of the telemetry
//!
//! A miner knows one number the network cannot compute for itself: how fast it
//! is hashing. Everything else on the dashboard — height, peers, difficulty —
//! a node can read off the chain. The hash rate has to come from the machines
//! doing the work, and this is what produces it.
//!
//! # Why a sliding window and not a running average
//!
//! A running average since startup is monotone in practice: a miner that has
//! been up for a day and then loses half its GPUs reports a number that barely
//! moves. The figure an operator wants is "what is this machine doing *now*",
//! and the figure the dashboard needs is one that falls when the hardware
//! does.
//!
//! So the meter keeps [`BUCKETS`] one-second buckets and reports the rate over
//! however many of them are populated. A miner that just started reports its
//! first second rather than nothing, and a miner that stops reports zero
//! within [`BUCKETS`] seconds.
//!
//! # It is cheap enough for the loop it lives in
//!
//! [`HashMeter::record`] is one relaxed atomic add on the common path. It is
//! called once per dispatch, not once per hash — a GPU dispatch is tens of
//! thousands of hashes, and a meter that needed a lock per batch would be
//! measuring itself.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// One-second buckets kept.
///
/// Thirty seconds: long enough that a slow dispatch does not read as a stall,
/// short enough that unplugging a card shows up before an operator has
/// finished looking at the page.
pub const BUCKETS: usize = 30;

/// A sliding-window hash rate meter.
///
/// Cheap to record into and safe to share: [`HashMeter::record`] takes
/// `&self`, so a miner can hand one `Arc` to every worker thread.
#[derive(Debug)]
pub struct HashMeter {
    /// Hashes in the bucket currently being filled.
    current: AtomicU64,
    /// The completed buckets, and which one is next.
    window: Mutex<Window>,
    /// Total hashes since the meter was made, for a lifetime figure.
    total: AtomicU64,
}

#[derive(Debug)]
struct Window {
    buckets: [u64; BUCKETS],
    /// How many buckets hold real data — less than [`BUCKETS`] until the
    /// window has filled once.
    filled: usize,
    next: usize,
    /// When the bucket being filled started.
    started: Instant,
}

impl HashMeter {
    /// A meter with an empty window.
    #[must_use]
    pub fn new() -> Self {
        Self::starting_at(Instant::now())
    }

    /// A meter whose first bucket started at `start`.
    ///
    /// The seam tests use to advance time without sleeping. A test that slept
    /// for thirty seconds to check a thirty-second window is a test that gets
    /// deleted.
    #[must_use]
    pub fn starting_at(start: Instant) -> Self {
        Self {
            current: AtomicU64::new(0),
            window: Mutex::new(Window {
                buckets: [0; BUCKETS],
                filled: 0,
                next: 0,
                started: start,
            }),
            total: AtomicU64::new(0),
        }
    }

    /// Records `hashes` completed.
    ///
    /// Call once per dispatch or per batch, not once per hash.
    pub fn record(&self, hashes: u64) {
        self.current.fetch_add(hashes, Ordering::Relaxed);
        self.total.fetch_add(hashes, Ordering::Relaxed);
    }

    /// Hashes since the meter was created.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// The rate over the populated window, in whole hashes per second.
    ///
    /// Rounded down. A miner that reports 0 rather than 0.4 h/s is a miner
    /// whose hardware is not working, and the distinction is not one anybody
    /// acts on differently.
    #[must_use]
    pub fn rate(&self) -> u64 {
        self.rate_at(Instant::now())
    }

    /// The rate as of `now`.
    #[must_use]
    pub fn rate_at(&self, now: Instant) -> u64 {
        let mut window = self.lock();
        self.roll(&mut window, now);

        if window.filled == 0 {
            // The first second is not over. Report the partial bucket rather
            // than zero: a miner that just started is not a miner that is
            // stopped, and reporting zero for the first thirty seconds after
            // every restart would put a dip in the network chart on every
            // deploy.
            let elapsed = now.saturating_duration_since(window.started);
            let hashes = self.current.load(Ordering::Relaxed);
            return rate_over(hashes, elapsed);
        }

        let hashes: u64 = window
            .buckets
            .iter()
            .take(window.filled)
            .copied()
            .fold(0u64, u64::saturating_add);

        // The partial bucket is deliberately excluded. Including it would
        // divide a fraction of a second's work by a whole second and make
        // every reading dip just after each roll.
        rate_over(hashes, Duration::from_secs(window.filled as u64))
    }

    /// Moves completed seconds into the window.
    fn roll(&self, window: &mut Window, now: Instant) {
        let mut elapsed = now.saturating_duration_since(window.started).as_secs();
        if elapsed == 0 {
            return;
        }

        // The first elapsed second closes the bucket that was being filled.
        let hashes = self.current.swap(0, Ordering::Relaxed);
        window.buckets[window.next] = hashes;
        window.next = (window.next + 1) % BUCKETS;
        window.filled = (window.filled + 1).min(BUCKETS);
        elapsed -= 1;

        // Any further elapsed seconds are seconds in which nothing was
        // recorded, and they are zeroes rather than gaps — a miner that
        // stopped must read as stopped, not as "still doing whatever it last
        // did".
        let idle = elapsed.min(BUCKETS as u64) as usize;
        for _ in 0..idle {
            window.buckets[window.next] = 0;
            window.next = (window.next + 1) % BUCKETS;
            window.filled = (window.filled + 1).min(BUCKETS);
        }

        window.started = now;
    }

    /// A poisoned lock means a thread panicked while reading the rate. The
    /// buckets are structurally intact, and losing telemetry is not a reason
    /// to take a miner down.
    fn lock(&self) -> std::sync::MutexGuard<'_, Window> {
        self.window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for HashMeter {
    fn default() -> Self {
        Self::new()
    }
}

/// `hashes / seconds`, rounded down, with a zero span reading as zero.
fn rate_over(hashes: u64, span: Duration) -> u64 {
    let seconds = span.as_secs_f64();
    if seconds <= 0.0 {
        return 0;
    }
    // `as u64` saturates on overflow and gives 0 for NaN, both of which are
    // the readings a caller wants from a meter that has gone wrong.
    (hashes as f64 / seconds) as u64
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn at(start: Instant, seconds: u64) -> Instant {
        start + Duration::from_secs(seconds)
    }

    /// Reads the rate at `seconds` and discards it.
    ///
    /// Reading is what rolls the window — the meter has no separate tick, and
    /// adding one would be a second way to advance time that could disagree
    /// with the first. This names the intent where a bare discarded call would
    /// only look like a forgotten assertion.
    fn advance(meter: &HashMeter, start: Instant, seconds: u64) {
        let _ = meter.rate_at(at(start, seconds));
    }

    #[test]
    fn a_steady_miner_reports_its_steady_rate() {
        let start = Instant::now();
        let meter = HashMeter::starting_at(start);

        for second in 0..10 {
            meter.record(1_000);
            // Reading at each second boundary is what a reporter does.
            advance(&meter, start, second + 1);
        }

        assert_eq!(meter.rate_at(at(start, 10)), 1_000);
    }

    #[test]
    fn a_miner_that_just_started_reports_a_rate_rather_than_zero() {
        // Reporting zero for the first thirty seconds after every restart
        // would put a dip in the network chart on every deploy.
        let start = Instant::now();
        let meter = HashMeter::starting_at(start);
        meter.record(500);

        assert_eq!(
            meter.rate_at(start + Duration::from_millis(500)),
            1_000,
            "500 hashes in half a second is 1,000 h/s"
        );
    }

    #[test]
    fn a_miner_that_stops_falls_to_zero_within_the_window() {
        // The property a running average does not have, and the reason the
        // window exists: unplugging a card has to be visible.
        let start = Instant::now();
        let meter = HashMeter::starting_at(start);

        for second in 0..BUCKETS as u64 {
            meter.record(1_000);
            advance(&meter, start, second + 1);
        }
        assert_eq!(meter.rate_at(at(start, BUCKETS as u64)), 1_000);

        // Nothing recorded for a full window.
        assert_eq!(
            meter.rate_at(at(start, BUCKETS as u64 * 2)),
            0,
            "a stopped miner must read as stopped"
        );
    }

    #[test]
    fn a_halved_rate_shows_up_as_halved() {
        // Losing half the GPUs. A since-startup average would barely move.
        let start = Instant::now();
        let meter = HashMeter::starting_at(start);

        for second in 0..BUCKETS as u64 {
            meter.record(1_000);
            advance(&meter, start, second + 1);
        }

        let mut now = BUCKETS as u64;
        for _ in 0..BUCKETS {
            meter.record(500);
            now += 1;
            advance(&meter, start, now);
        }

        assert_eq!(meter.rate_at(at(start, now)), 500);
    }

    #[test]
    fn idle_seconds_are_zeroes_and_not_gaps() {
        // A miner that recorded nothing for ten seconds did zero hashes in
        // them. Skipping the buckets would report the last known rate forever.
        let start = Instant::now();
        let meter = HashMeter::starting_at(start);

        meter.record(3_000);
        advance(&meter, start, 1);
        assert_eq!(meter.rate_at(at(start, 1)), 3_000);

        // Ten silent seconds: 3,000 hashes over eleven buckets.
        let rate = meter.rate_at(at(start, 11));
        assert!(rate < 300, "the rate must fall through the silence: {rate}");
    }

    #[test]
    fn the_window_never_grows_past_its_bound() {
        let start = Instant::now();
        let meter = HashMeter::starting_at(start);

        // A very long gap must not walk the ring more than once.
        meter.record(1_000);
        advance(&meter, start, 100_000);
        assert_eq!(meter.rate_at(at(start, 100_000)), 0);
    }

    #[test]
    fn the_total_is_a_lifetime_figure_and_does_not_decay() {
        let start = Instant::now();
        let meter = HashMeter::starting_at(start);

        meter.record(1_000);
        advance(&meter, start, BUCKETS as u64 * 2);

        assert_eq!(meter.rate_at(at(start, BUCKETS as u64 * 2)), 0);
        assert_eq!(
            meter.total(),
            1_000,
            "the window decays; the total does not"
        );
    }

    #[test]
    fn a_meter_can_be_shared_between_worker_threads() {
        // The shape a multi-GPU miner uses: one meter, one `Arc` per worker.
        use std::sync::Arc;

        let start = Instant::now();
        let meter = Arc::new(HashMeter::starting_at(start));

        let workers: Vec<_> = (0..8)
            .map(|_| {
                let meter = Arc::clone(&meter);
                std::thread::spawn(move || {
                    for _ in 0..1_000 {
                        meter.record(10);
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("no worker panicked");
        }

        assert_eq!(meter.total(), 8 * 1_000 * 10);
    }
}
