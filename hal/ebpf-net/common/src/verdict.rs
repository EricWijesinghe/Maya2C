//! The order in which the kernel refuses a relay datagram.
//!
//! The XDP program does the map lookups, copies the header off the packet, and
//! calls [`judge`]; everything that decides the outcome is here, where it runs
//! under the host's test suite rather than only under a kernel.
//!
//! The order is cheapest refusal first, and it is chosen so that no step can be
//! skipped by the sender:
//!
//! 1. **Blocked source.** One hash lookup. A blocked source spends no tokens,
//!    so its flood cannot push honest sources' buckets out of the LRU map.
//! 2. **Rate.** Before the header is examined, so a flood of garbage costs the
//!    sender its tokens exactly as a flood of well-formed chunks does.
//! 3. **Structure.** [`RelayHeader::check_datagram`]: fields, and the exact
//!    length they imply.
//!
//! Nothing here reads a key. Whether the datagram came from a peer holding one
//! is decided after redirection, in user space.

use crate::header::{HEADER_LEN, RelayHeader};
use crate::maps::{BlockEntry, Counter};
use crate::rate::{Bucket, RateLimit};

/// What happens to one relay datagram.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Hand it to the `AF_XDP` socket on its queue.
    Redirect,
    /// Drop it at the driver, counting it under the given counter.
    Drop(Counter),
}

/// A verdict, and the bucket to store for the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Judgement {
    /// What to do with the datagram.
    pub verdict: Verdict,
    /// The bucket to write back; `None` leaves the rate map untouched.
    pub bucket: Option<Bucket>,
}

/// Decides one relay datagram.
///
/// - `blocked`: the source's blocklist entry, if any.
/// - `bucket`: the source's rate bucket, if any.
/// - `header`: the first [`HEADER_LEN`] bytes of the UDP payload, or `None` if
///   the payload is shorter than that.
/// - `udp_payload_len`: the whole UDP payload's length.
///
/// Always inlined. A BPF call passes at most five arguments in registers, and
/// this takes six: when LLVM emitted it as a real call it spilled the sixth
/// through R11, and the kernel refused the program ("R11 is invalid").
#[must_use]
#[inline(always)]
pub fn judge(
    blocked: Option<BlockEntry>,
    bucket: Option<Bucket>,
    limit: RateLimit,
    now_ns: u64,
    header: Option<&[u8; HEADER_LEN]>,
    udp_payload_len: usize,
) -> Judgement {
    if let Some(entry) = blocked
        && entry.is_active(now_ns)
    {
        return Judgement {
            verdict: Verdict::Drop(Counter::Blocked),
            bucket: None,
        };
    }

    let (bucket, admitted) = match bucket {
        Some(bucket) => {
            let (next, admitted) = limit.admit(bucket, now_ns);
            (Some(next), admitted)
        }
        None => match limit.first(now_ns) {
            Some(first) => (Some(first), true),
            None => (None, false),
        },
    };
    if !admitted {
        return Judgement {
            verdict: Verdict::Drop(Counter::RateLimited),
            bucket,
        };
    }

    let verdict = match header {
        Some(bytes) if RelayHeader::check_datagram(bytes, udp_payload_len).is_ok() => {
            Verdict::Redirect
        }
        _ => Verdict::Drop(Counter::Malformed),
    };
    Judgement { verdict, bucket }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const LIMIT: RateLimit = RateLimit {
        burst: 2,
        per_second: 0,
    };

    fn valid() -> ([u8; HEADER_LEN], usize) {
        let header = RelayHeader::new(1, [3; 32], 0, 100).unwrap();
        (header.encode(), header.datagram_len())
    }

    #[test]
    fn a_well_formed_datagram_from_a_new_source_is_redirected() {
        let (header, len) = valid();
        let judgement = judge(None, None, LIMIT, 0, Some(&header), len);
        assert_eq!(judgement.verdict, Verdict::Redirect);
        assert_eq!(judgement.bucket.map(|b| b.tokens), Some(1));
    }

    #[test]
    fn a_blocked_source_is_dropped_first_and_spends_no_tokens() {
        let (header, len) = valid();
        let blocked = Some(BlockEntry { expires_ns: 10 });
        let judgement = judge(blocked, None, LIMIT, 5, Some(&header), len);
        assert_eq!(judgement.verdict, Verdict::Drop(Counter::Blocked));
        assert_eq!(judgement.bucket, None);
    }

    #[test]
    fn a_lapsed_block_no_longer_applies() {
        let (header, len) = valid();
        let lapsed = Some(BlockEntry { expires_ns: 10 });
        let judgement = judge(lapsed, None, LIMIT, 10, Some(&header), len);
        assert_eq!(judgement.verdict, Verdict::Redirect);
    }

    #[test]
    fn garbage_spends_tokens_so_a_malformed_flood_is_rate_limited_too() {
        let mut bucket = None;
        let verdicts: Vec<Verdict> = (0..3)
            .map(|_| {
                let judgement = judge(None, bucket, LIMIT, 0, None, 3);
                bucket = judgement.bucket;
                judgement.verdict
            })
            .collect();
        assert_eq!(
            verdicts,
            [
                Verdict::Drop(Counter::Malformed),
                Verdict::Drop(Counter::Malformed),
                Verdict::Drop(Counter::RateLimited),
            ]
        );
    }

    #[test]
    fn a_header_that_misstates_the_datagram_length_is_malformed() {
        let (header, len) = valid();
        let judgement = judge(None, None, LIMIT, 0, Some(&header), len + 1);
        assert_eq!(judgement.verdict, Verdict::Drop(Counter::Malformed));
    }

    #[test]
    fn a_limit_that_admits_nothing_stores_no_bucket() {
        let (header, len) = valid();
        let closed = RateLimit {
            burst: 0,
            per_second: 0,
        };
        let judgement = judge(None, None, closed, 0, Some(&header), len);
        assert_eq!(judgement.verdict, Verdict::Drop(Counter::RateLimited));
        assert_eq!(judgement.bucket, None);
    }
}
