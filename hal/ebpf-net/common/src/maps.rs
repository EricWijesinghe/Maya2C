//! The records the XDP program stores, and the names user space finds them by.
//!
//! Every record is `#[repr(C)]`, built from fixed-width integers, and has no
//! implicit padding — asserted by size below — so user space and the kernel
//! read the same bytes and no uninitialised byte ever reaches a map.

use crate::rate::{Bucket, RateLimit};

/// IPv4 sources whose relay datagrams are dropped. Key: the address as the
/// four bytes on the wire, read as a native-endian `u32`.
pub const BLOCK_V4: &str = "BLOCK_V4";
/// IPv6 sources whose relay datagrams are dropped. Key: `[u8; 16]`.
pub const BLOCK_V6: &str = "BLOCK_V6";
/// Token buckets for IPv4 sources. LRU, so a spoofed-source flood evicts old
/// entries instead of filling the map.
pub const RATE_V4: &str = "RATE_V4";
/// Token buckets for IPv6 sources.
pub const RATE_V6: &str = "RATE_V6";
/// `AF_XDP` sockets, indexed by receive queue.
pub const XSKS: &str = "XSKS";
/// The single [`Config`] record, at index 0.
pub const CONFIG: &str = "CONFIG";
/// Per-CPU [`Counter`] slots.
pub const COUNTERS: &str = "COUNTERS";
/// The XDP program's function name.
pub const PROGRAM: &str = "maya_relay";

/// Blocklist capacity per address family.
pub const BLOCKLIST_CAPACITY: u32 = 65_536;
/// Rate-bucket capacity per address family.
pub const RATE_CAPACITY: u32 = 262_144;
/// Receive queues an accelerator can serve.
pub const QUEUE_CAPACITY: u32 = 64;

/// A blocked source.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockEntry {
    /// `CLOCK_MONOTONIC` nanoseconds at which the block lapses.
    ///
    /// Lapsing in the kernel as well as being removed from user space, so a
    /// node that crashes between a quarantine and its release does not leave a
    /// peer blocked until reboot.
    pub expires_ns: u64,
}

impl BlockEntry {
    /// Whether the block still applies at `now_ns`.
    #[must_use]
    #[inline(always)]
    pub const fn is_active(self, now_ns: u64) -> bool {
        now_ns < self.expires_ns
    }
}

/// What the XDP program is configured with.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// UDP destination port of relay traffic, in host order. Zero disables the
    /// program: everything passes.
    pub relay_port: u16,
    /// Always zero.
    pub reserved: u16,
    /// Per-source admission.
    pub limit: RateLimit,
}

impl Config {
    /// A configuration for `relay_port` under `limit`.
    #[must_use]
    pub const fn new(relay_port: u16, limit: RateLimit) -> Self {
        Self {
            relay_port,
            reserved: 0,
            limit,
        }
    }
}

/// What the XDP program counts. The discriminant is the slot.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Counter {
    /// Not relay traffic, handed to the kernel stack untouched.
    Passed = 0,
    /// Relay traffic that failed a structural check.
    Malformed = 1,
    /// Relay traffic from a blocked source.
    Blocked = 2,
    /// Relay traffic from a source over its rate.
    RateLimited = 3,
    /// Relay traffic redirected to an `AF_XDP` socket.
    Redirected = 4,
    /// Relay traffic with no socket bound on its queue; dropped.
    RedirectFailed = 5,
}

/// Number of counter slots.
pub const COUNTER_SLOTS: u32 = 6;

impl Counter {
    /// Every counter, in slot order.
    pub const ALL: [Self; COUNTER_SLOTS as usize] = [
        Self::Passed,
        Self::Malformed,
        Self::Blocked,
        Self::RateLimited,
        Self::Redirected,
        Self::RedirectFailed,
    ];

    /// This counter's slot.
    #[must_use]
    pub const fn slot(self) -> u32 {
        self as u32
    }

    /// A stable name for metrics.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Malformed => "malformed",
            Self::Blocked => "blocked",
            Self::RateLimited => "rate_limited",
            Self::Redirected => "redirected",
            Self::RedirectFailed => "redirect_failed",
        }
    }
}

const _: () = assert!(core::mem::size_of::<BlockEntry>() == 8);
const _: () = assert!(core::mem::size_of::<Bucket>() == 16);
const _: () = assert!(core::mem::size_of::<RateLimit>() == 8);
const _: () = assert!(core::mem::size_of::<Config>() == 12);

#[cfg(all(feature = "aya", target_os = "linux"))]
#[allow(unsafe_code)]
mod pod {
    use super::{BlockEntry, Bucket, Config};

    // SAFETY: `#[repr(C)]`, only `u64`; every bit pattern is a valid value.
    unsafe impl aya::Pod for BlockEntry {}
    // SAFETY: `#[repr(C)]`, `u32, u32, u64` with no padding (size asserted as
    // 16 above); every bit pattern is a valid value.
    unsafe impl aya::Pod for Bucket {}
    // SAFETY: `#[repr(C)]`, `u16, u16, u32, u32` with no padding (size asserted
    // as 12 above); every bit pattern is a valid value.
    unsafe impl aya::Pod for Config {}
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn counter_slots_are_dense_and_in_order() {
        for (slot, counter) in Counter::ALL.iter().enumerate() {
            assert_eq!(counter.slot() as usize, slot);
        }
    }

    #[test]
    fn map_names_fit_the_kernel_object_name_limit() {
        // BPF_OBJ_NAME_LEN is 16 including the terminator.
        for name in [
            BLOCK_V4, BLOCK_V6, RATE_V4, RATE_V6, XSKS, CONFIG, COUNTERS, PROGRAM,
        ] {
            assert!(name.len() <= 15, "{name}");
        }
    }

    #[test]
    fn a_block_lapses_at_its_expiry() {
        let entry = BlockEntry { expires_ns: 100 };
        assert!(entry.is_active(99));
        assert!(!entry.is_active(100));
    }
}
