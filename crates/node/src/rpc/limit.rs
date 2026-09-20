//! Per-peer RPC rate limiting.
//!
//! # Why this exists
//!
//! Before it, there was no rate limiting anywhere in `src/rpc` or
//! `src/network`. A `config.toml` field naming a limit over code that did not
//! enforce one would be worse than no field at all: an operator reads it, sets
//! it, and believes they are protected.
//!
//! # What it protects against, and what it does not
//!
//! It bounds how fast **one address** can make this node work. That is the
//! cheap attack: a single client issuing `get_block_by_height` in a loop makes
//! a node read and serialise blocks instead of validating them, and costs the
//! attacker almost nothing.
//!
//! It is **not** a defence against a distributed flood. A thousand addresses at
//! the limit each are a thousand times the limit, and no per-peer bucket can
//! see that. That needs a global ceiling or something in front of the node, and
//! `maya-api-gateway` is the layer where that belongs.
//!
//! Stating the boundary matters more than the feature: a limiter described as
//! "rate limiting" without it invites someone to treat the node's RPC as
//! publicly exposable, which it is not.
//!
//! # A token bucket, not a fixed window
//!
//! A fixed window admits twice the rate across a boundary — 50 requests at
//! 0.999 s and 50 more at 1.001 s is 100 in two milliseconds, and every
//! attacker knows it. A bucket refills continuously, so the sustained rate is
//! the sustained rate and the burst is stated explicitly.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Instant;

/// Addresses tracked before the table is swept.
///
/// A bucket per address is a map an attacker can grow by sourcing from many
/// addresses, so the table itself needs a ceiling. When it is reached, entries
/// that have refilled to full are dropped: a full bucket is indistinguishable
/// from an absent one, so forgetting it changes no decision.
const MAX_TRACKED: usize = 65_536;

/// One address's bucket.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    /// Tokens available, as a float so partial refills are not lost.
    ///
    /// Integer tokens with integer division would round every refill below one
    /// token down to zero, and a client polling faster than one token's worth
    /// of interval would be throttled to nothing.
    tokens: f64,
    /// When `tokens` was last brought up to date.
    updated: Instant,
}

/// A per-address token-bucket limiter.
#[derive(Debug)]
pub struct RateLimiter {
    /// Sustained requests per second. Zero disables the limiter entirely.
    rate: f64,
    /// Bucket capacity, in tokens.
    burst: f64,
    buckets: Mutex<HashMap<IpAddr, Bucket>>,
}

impl RateLimiter {
    /// Builds a limiter.
    ///
    /// `rate` of zero disables it: [`RateLimiter::check`] then always allows,
    /// and no bucket is ever allocated. That is the documented way to turn the
    /// limiter off, and it costs nothing when off.
    #[must_use]
    pub fn new(rate_per_second: u32, burst: u32) -> Self {
        Self {
            rate: f64::from(rate_per_second),
            burst: f64::from(burst.max(rate_per_second)),
            buckets: Mutex::new(HashMap::new()),
        }
    }

    /// Whether the limiter is doing anything.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.rate > 0.0
    }

    /// Takes a token for `address`, reporting whether one was available.
    ///
    /// # Panics
    ///
    /// Never in practice: the mutex is only held for the map update, and no
    /// code inside can panic while holding it. A poisoned lock is recovered
    /// from rather than propagated, because a limiter that stopped working
    /// would silently remove the protection it exists to provide.
    #[must_use]
    pub fn check(&self, address: IpAddr) -> bool {
        self.check_at(address, Instant::now())
    }

    /// [`RateLimiter::check`] against a caller-supplied clock.
    ///
    /// Time is a parameter so the tests can advance it. A rate limiter tested
    /// with `sleep` is a slow test that is flaky on a loaded machine, and the
    /// property worth checking — that the bucket refills at exactly the
    /// configured rate — is not observable through real time anyway.
    #[must_use]
    pub fn check_at(&self, address: IpAddr, now: Instant) -> bool {
        if !self.is_enabled() {
            return true;
        }

        let mut buckets = match self.buckets.lock() {
            Ok(guard) => guard,
            // A poisoned lock means some other thread panicked mid-update. The
            // map is still structurally sound, and refusing to limit is worse
            // than continuing with it.
            Err(poisoned) => poisoned.into_inner(),
        };

        if buckets.len() >= MAX_TRACKED && !buckets.contains_key(&address) {
            Self::sweep(&mut buckets, self.rate, self.burst, now);
        }

        let bucket = buckets.entry(address).or_insert(Bucket {
            tokens: self.burst,
            updated: now,
        });

        // Refill for the elapsed time, capped at the burst.
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.rate).min(self.burst);
        bucket.updated = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Drops entries that have refilled to capacity.
    ///
    /// A full bucket and an absent one produce the same answer for every
    /// subsequent request, so forgetting a full bucket is free. This is what
    /// keeps the table from being a memory-growth vector.
    fn sweep(buckets: &mut HashMap<IpAddr, Bucket>, rate: f64, burst: f64, now: Instant) {
        buckets.retain(|_, bucket| {
            let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
            bucket.tokens + elapsed * rate < burst
        });
    }

    /// Addresses currently tracked. For tests and metrics.
    #[must_use]
    pub fn tracked(&self) -> usize {
        match self.buckets.lock() {
            Ok(guard) => guard.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::time::Duration;

    fn addr(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    #[test]
    fn a_zero_rate_allows_everything_and_allocates_nothing() {
        let limiter = RateLimiter::new(0, 0);
        assert!(!limiter.is_enabled());
        for _ in 0..1000 {
            assert!(limiter.check(addr(1)));
        }
        assert_eq!(limiter.tracked(), 0, "a disabled limiter must not allocate");
    }

    #[test]
    fn a_burst_is_allowed_and_then_refused() {
        let limiter = RateLimiter::new(10, 20);
        let now = Instant::now();

        for i in 0..20 {
            assert!(limiter.check_at(addr(1), now), "burst token {i} refused");
        }
        assert!(
            !limiter.check_at(addr(1), now),
            "the 21st request within one instant must be refused"
        );
    }

    #[test]
    fn the_bucket_refills_at_the_configured_rate() {
        // The property a fixed window does not have. Ten per second means one
        // token per hundred milliseconds, continuously.
        let limiter = RateLimiter::new(10, 10);
        let start = Instant::now();

        for _ in 0..10 {
            assert!(limiter.check_at(addr(1), start));
        }
        assert!(!limiter.check_at(addr(1), start));

        // Half a second later: five tokens.
        let later = start + Duration::from_millis(500);
        for i in 0..5 {
            assert!(limiter.check_at(addr(1), later), "refilled token {i}");
        }
        assert!(!limiter.check_at(addr(1), later));
    }

    #[test]
    fn refill_never_exceeds_the_burst() {
        // An idle client must not accumulate an unbounded allowance and then
        // spend it all at once — which is exactly the behaviour a naive
        // "requests since last check" counter has.
        let limiter = RateLimiter::new(10, 20);
        let start = Instant::now();
        assert!(limiter.check_at(addr(1), start));

        let much_later = start + Duration::from_secs(3600);
        for i in 0..20 {
            assert!(limiter.check_at(addr(1), much_later), "token {i}");
        }
        assert!(
            !limiter.check_at(addr(1), much_later),
            "an hour idle must still grant only the burst"
        );
    }

    #[test]
    fn one_address_cannot_exhaust_another() {
        // Per-peer is the whole point: a noisy client must not throttle a quiet
        // one.
        let limiter = RateLimiter::new(5, 5);
        let now = Instant::now();

        for _ in 0..5 {
            assert!(limiter.check_at(addr(1), now));
        }
        assert!(!limiter.check_at(addr(1), now));

        for i in 0..5 {
            assert!(
                limiter.check_at(addr(2), now),
                "a second address must have its own bucket, token {i}"
            );
        }
    }

    #[test]
    fn a_burst_below_the_rate_is_raised_to_it() {
        // `NodeConfig::validate` refuses this combination, but the limiter is a
        // public type and a caller could construct it directly. A bucket
        // smaller than one second of refill would throttle below its own
        // documented rate.
        let limiter = RateLimiter::new(10, 2);
        let now = Instant::now();
        for i in 0..10 {
            assert!(limiter.check_at(addr(1), now), "token {i}");
        }
    }

    #[test]
    fn the_table_does_not_grow_without_bound() {
        // A bucket per address is a map an attacker grows by sourcing widely.
        // Full buckets are indistinguishable from absent ones, so the sweep
        // drops them and the table stays bounded.
        let limiter = RateLimiter::new(1000, 1000);
        let start = Instant::now();

        for index in 0..300u32 {
            let ip = IpAddr::from([10, 0, (index >> 8) as u8, (index & 0xff) as u8]);
            assert!(limiter.check_at(ip, start));
        }
        assert_eq!(limiter.tracked(), 300);

        // Every bucket has refilled; a sweep would drop all of them.
        let later = start + Duration::from_secs(10);
        let mut buckets = limiter.buckets.lock().expect("not poisoned");
        RateLimiter::sweep(&mut buckets, limiter.rate, limiter.burst, later);
        assert!(
            buckets.is_empty(),
            "refilled buckets carry no information and must be dropped"
        );
    }

    #[test]
    fn a_partially_drained_bucket_survives_a_sweep() {
        // The other half: a bucket that is still limiting somebody must not be
        // forgotten, or the sweep would reset an attacker's allowance.
        let limiter = RateLimiter::new(1, 1);
        let start = Instant::now();
        assert!(limiter.check_at(addr(1), start));

        let mut buckets = limiter.buckets.lock().expect("not poisoned");
        RateLimiter::sweep(&mut buckets, limiter.rate, limiter.burst, start);
        assert_eq!(buckets.len(), 1, "a drained bucket must be kept");
    }
}
