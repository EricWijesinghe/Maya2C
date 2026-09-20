//! A per-source token bucket, in integers.
//!
//! Integers because the kernel has no floating point, and because this is the
//! same function whether it runs in the XDP program or in a test. No division
//! by a value that could be zero without a checked branch: a panic path is a
//! loop to the verifier, and the verifier refuses loops it cannot bound.

use crate::header::MAX_CHUNKS;

/// Nanoseconds in a second.
pub const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// How much a single source may send.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimit {
    /// Tokens a quiet source accumulates, and so the largest burst it may send.
    pub burst: u32,
    /// Tokens earned per second.
    pub per_second: u32,
}

/// One source's bucket, as stored in the kernel's rate map.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bucket {
    /// Tokens available now.
    pub tokens: u32,
    /// Always zero. Explicit, so the record has no uninitialised padding.
    pub reserved: u32,
    /// The monotonic time, in nanoseconds, that `tokens` is accurate as of.
    pub refilled_at_ns: u64,
}

impl RateLimit {
    /// Two of the largest blocks at once, then about one chunk per millisecond.
    ///
    /// The burst is what matters: a peer relaying one maximal block sends
    /// [`MAX_CHUNKS`] datagrams in a few milliseconds, and a limit below that
    /// would refuse an honest block. Sustained, blocks arrive every fifteen
    /// seconds, so a refill of 1,024 a second restores a full burst between
    /// them with room to spare.
    pub const DEFAULT: Self = Self {
        burst: 2 * MAX_CHUNKS,
        per_second: 1_024,
    };

    /// The bucket for a source seen for the first time, having spent the token
    /// for the datagram that introduced it. `None` if the limit admits nothing.
    #[must_use]
    #[inline(always)]
    pub const fn first(self, now_ns: u64) -> Option<Bucket> {
        if self.burst == 0 {
            return None;
        }
        Some(Bucket {
            tokens: self.burst - 1,
            reserved: 0,
            refilled_at_ns: now_ns,
        })
    }

    /// Refills `bucket` to `now_ns` and spends one token if there is one.
    ///
    /// Returns the bucket to store back, and whether the datagram is admitted.
    /// A refused datagram still returns a refilled bucket, so the refill work is
    /// not repeated on the next one.
    #[must_use]
    #[inline(always)]
    pub fn admit(self, bucket: Bucket, now_ns: u64) -> (Bucket, bool) {
        let refilled = self.refill(bucket, now_ns);
        if refilled.tokens == 0 {
            return (refilled, false);
        }
        let spent = Bucket {
            tokens: refilled.tokens - 1,
            ..refilled
        };
        (spent, true)
    }

    #[inline(always)]
    fn refill(self, bucket: Bucket, now_ns: u64) -> Bucket {
        let per_second = u64::from(self.per_second);
        // `None` only when per_second is zero, which never refills.
        let Some(longest) = u64::MAX.checked_div(per_second) else {
            return bucket;
        };
        // A clock that steps backwards earns nothing rather than wrapping.
        let elapsed = now_ns.saturating_sub(bucket.refilled_at_ns);
        let elapsed = if elapsed < longest { elapsed } else { longest };
        let earned = elapsed * per_second / NANOS_PER_SECOND;
        if earned == 0 {
            return bucket;
        }

        let tokens = u64::from(bucket.tokens).saturating_add(earned);
        if tokens >= u64::from(self.burst) {
            return Bucket {
                tokens: self.burst,
                reserved: 0,
                refilled_at_ns: now_ns,
            };
        }
        // Advance the clock by what the earned tokens cost, not to `now_ns`, so
        // a fraction of a token carries over instead of being lost on every
        // datagram. `earned * NANOS <= elapsed * per_second`, so no overflow.
        let cost = match (earned * NANOS_PER_SECOND).checked_div(per_second) {
            Some(cost) => cost,
            None => elapsed,
        };
        Bucket {
            tokens: u32::try_from(tokens).unwrap_or(self.burst),
            reserved: 0,
            refilled_at_ns: bucket.refilled_at_ns.saturating_add(cost),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const SECOND: u64 = NANOS_PER_SECOND;

    #[test]
    fn a_burst_is_admitted_and_the_next_datagram_is_not() {
        let limit = RateLimit {
            burst: 3,
            per_second: 1,
        };
        let mut bucket = limit.first(0).unwrap();
        for _ in 0..2 {
            let (next, admitted) = limit.admit(bucket, 0);
            assert!(admitted);
            bucket = next;
        }
        let (bucket, admitted) = limit.admit(bucket, 0);
        assert!(!admitted);
        assert_eq!(bucket.tokens, 0);
    }

    #[test]
    fn tokens_refill_at_the_configured_rate_and_stop_at_the_burst() {
        let limit = RateLimit {
            burst: 10,
            per_second: 4,
        };
        let empty = Bucket {
            tokens: 0,
            reserved: 0,
            refilled_at_ns: 0,
        };
        let (after_one_second, admitted) = limit.admit(empty, SECOND);
        assert!(admitted);
        assert_eq!(after_one_second.tokens, 3);

        let (after_an_hour, _) = limit.admit(empty, 3_600 * SECOND);
        assert_eq!(after_an_hour.tokens, 9);
    }

    #[test]
    fn a_fraction_of_a_token_carries_over_between_datagrams() {
        let limit = RateLimit {
            burst: 5,
            per_second: 1,
        };
        let empty = Bucket {
            tokens: 0,
            reserved: 0,
            refilled_at_ns: 0,
        };
        let (half, admitted) = limit.admit(empty, SECOND / 2);
        assert!(!admitted);
        assert_eq!(
            half.refilled_at_ns, 0,
            "no token earned, so no time consumed"
        );
        let (_, admitted) = limit.admit(half, SECOND);
        assert!(admitted, "two half-seconds are one token");
    }

    #[test]
    fn a_clock_that_steps_backwards_earns_nothing_and_does_not_wrap() {
        let limit = RateLimit::DEFAULT;
        let bucket = Bucket {
            tokens: 0,
            reserved: 0,
            refilled_at_ns: 10 * SECOND,
        };
        let (after, admitted) = limit.admit(bucket, SECOND);
        assert!(!admitted);
        assert_eq!(after, bucket);
    }

    #[test]
    fn a_zero_rate_never_refills_and_a_zero_burst_admits_nothing() {
        let frozen = RateLimit {
            burst: 2,
            per_second: 0,
        };
        let bucket = frozen.first(0).unwrap();
        let (bucket, admitted) = frozen.admit(bucket, u64::MAX);
        assert!(admitted);
        let (_, admitted) = frozen.admit(bucket, u64::MAX);
        assert!(!admitted);

        let closed = RateLimit {
            burst: 0,
            per_second: 1_000,
        };
        assert_eq!(closed.first(0), None);
    }

    #[test]
    fn extreme_elapsed_time_and_rate_do_not_overflow() {
        let limit = RateLimit {
            burst: u32::MAX,
            per_second: u32::MAX,
        };
        let bucket = Bucket {
            tokens: 0,
            reserved: 0,
            refilled_at_ns: 0,
        };
        let (after, admitted) = limit.admit(bucket, u64::MAX);
        assert!(admitted);
        assert_eq!(after.tokens, u32::MAX - 1);
    }

    #[test]
    fn the_default_admits_one_maximal_block_in_a_single_burst() {
        let limit = RateLimit::DEFAULT;
        let mut bucket = limit.first(0).unwrap();
        for _ in 1..MAX_CHUNKS {
            let (next, admitted) = limit.admit(bucket, 0);
            assert!(admitted);
            bucket = next;
        }
    }
}
