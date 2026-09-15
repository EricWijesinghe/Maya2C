//! The XDP program: relay datagrams judged at the driver.
//!
//! For every frame on the interface:
//!
//! ```text
//! not IPv4/IPv6 UDP to the relay port ─────────────► XDP_PASS (untouched)
//! relay datagram ─► blocked? ─► over rate? ─► header + exact length?
//!                     │ drop       │ drop        │ drop      └─► AF_XDP socket
//! ```
//!
//! Everything that is not certainly relay traffic passes, so a node's SSH,
//! libp2p TCP, and anything else keep working with the program attached. The
//! decision itself is `maya_ebpf_net_common::verdict::judge`, the function the
//! host test suite exercises; this file only finds the fields and performs the
//! lookups, with the bounds checks the verifier requires.
//!
//! # Licence string
//!
//! The kernel asks every program for a licence and withholds GPL-only helpers
//! from one that is not GPL-compatible. This program calls only map lookups and
//! updates, `bpf_ktime_get_ns`, and `bpf_redirect_map`, none of which are
//! GPL-only, and declares no GPL licence. Adding a GPL-only helper — `aya-log`'s
//! perf output is one — therefore fails at load time with the kernel naming
//! it, instead of silently changing what licence this object is under. See
//! `docs/ebpf-net.md`.

#![no_std]
#![no_main]

use core::mem::size_of;

use aya_ebpf::bindings::xdp_action;
use aya_ebpf::helpers::bpf_ktime_get_ns;
use aya_ebpf::macros::{map, xdp};
use aya_ebpf::maps::{Array, HashMap, LruHashMap, PerCpuArray, XskMap};
use aya_ebpf::programs::XdpContext;
use maya_ebpf_net_common::header::HEADER_LEN;
use maya_ebpf_net_common::maps::{
    BLOCKLIST_CAPACITY, BlockEntry, COUNTER_SLOTS, Config, Counter, QUEUE_CAPACITY, RATE_CAPACITY,
};
use maya_ebpf_net_common::packet::{
    ETH_HEADER_LEN, ETHERTYPE_IPV4, ETHERTYPE_IPV6, IPPROTO_UDP, IPV4_MIN_HEADER_LEN,
    IPV6_HEADER_LEN, UDP_HEADER_LEN,
};
use maya_ebpf_net_common::rate::Bucket;
use maya_ebpf_net_common::verdict::{Verdict, judge};

const IPV4_MORE_FRAGMENTS: u16 = 0x2000;
const IPV4_FRAGMENT_OFFSET: u16 = 0x1fff;

#[map]
static CONFIG: Array<Config> = Array::with_max_entries(1, 0);

#[map]
static BLOCK_V4: HashMap<u32, BlockEntry> = HashMap::with_max_entries(BLOCKLIST_CAPACITY, 0);

#[map]
static BLOCK_V6: HashMap<[u8; 16], BlockEntry> = HashMap::with_max_entries(BLOCKLIST_CAPACITY, 0);

#[map]
static RATE_V4: LruHashMap<u32, Bucket> = LruHashMap::with_max_entries(RATE_CAPACITY, 0);

#[map]
static RATE_V6: LruHashMap<[u8; 16], Bucket> = LruHashMap::with_max_entries(RATE_CAPACITY, 0);

#[map]
static XSKS: XskMap = XskMap::with_max_entries(QUEUE_CAPACITY, 0);

#[map]
static COUNTERS: PerCpuArray<u64> = PerCpuArray::with_max_entries(COUNTER_SLOTS, 0);

/// Not a GPL-compatible string, deliberately: see the module documentation.
#[unsafe(no_mangle)]
#[unsafe(link_section = "license")]
pub static LICENSE: [u8; 12] = *b"Proprietary\0";

/// Entry point. The name is `maya_ebpf_net_common::maps::PROGRAM`.
#[xdp]
pub fn maya_relay(ctx: XdpContext) -> u32 {
    match relay(&ctx) {
        Some(action) => action,
        None => {
            count(Counter::Passed);
            xdp_action::XDP_PASS
        }
    }
}

/// `Some(action)` for relay traffic; `None` for everything to pass.
fn relay(ctx: &XdpContext) -> Option<u32> {
    let config = *CONFIG.get(0)?;
    if config.relay_port == 0 {
        return None;
    }
    let eth: [u8; ETH_HEADER_LEN] = read(ctx, 0)?;
    match u16::from_be_bytes([eth[12], eth[13]]) {
        ETHERTYPE_IPV4 => ipv4(ctx, &config),
        ETHERTYPE_IPV6 => ipv6(ctx, &config),
        _ => None,
    }
}

fn ipv4(ctx: &XdpContext, config: &Config) -> Option<u32> {
    let ip: [u8; IPV4_MIN_HEADER_LEN] = read(ctx, ETH_HEADER_LEN)?;
    if ip[0] >> 4 != 4 || ip[9] != IPPROTO_UDP {
        return None;
    }
    let header_len = usize::from(ip[0] & 0x0f) * 4;
    let flags = u16::from_be_bytes([ip[6], ip[7]]);
    if header_len < IPV4_MIN_HEADER_LEN || flags & IPV4_FRAGMENT_OFFSET != 0 {
        return None;
    }
    let udp_offset = ETH_HEADER_LEN + header_len;
    let udp: [u8; UDP_HEADER_LEN] = read(ctx, udp_offset)?;
    if u16::from_be_bytes([udp[2], udp[3]]) != config.relay_port {
        return None;
    }
    // Every read before any length field is compared with the packet: see `read`.
    let head: Option<[u8; HEADER_LEN]> = read(ctx, udp_offset + UDP_HEADER_LEN);
    let total_len = usize::from(u16::from_be_bytes([ip[2], ip[3]]));
    if total_len < header_len + UDP_HEADER_LEN || !fits(ctx, ETH_HEADER_LEN + total_len) {
        return None;
    }

    // Relay traffic from here: every path returns a verdict.
    let source = u32::from_ne_bytes([ip[12], ip[13], ip[14], ip[15]]);
    let payload_len = udp_payload_len(
        &udp,
        total_len - header_len,
        flags & IPV4_MORE_FRAGMENTS != 0,
    );
    let header = relay_header(head, payload_len);
    // SAFETY: the entry is copied out at once and no reference to it is kept,
    // so a concurrent update can at worst be read torn — which `judge` treats
    // as any other stale value, and the next datagram corrects.
    let blocked = unsafe { BLOCK_V4.get(&source) }.copied();
    // SAFETY: as above.
    let bucket = unsafe { RATE_V4.get(&source) }.copied();
    let judgement = judge(
        blocked,
        bucket,
        config.limit,
        now_ns(),
        header.as_ref(),
        payload_len.unwrap_or(0),
    );
    if let Some(next) = judgement.bucket {
        // A full LRU map evicts rather than refusing, so this cannot fail for
        // want of room; any other failure leaves the old bucket, which is safe.
        let _ = RATE_V4.insert(&source, &next, 0);
    }
    Some(act(ctx, judgement.verdict))
}

fn ipv6(ctx: &XdpContext, config: &Config) -> Option<u32> {
    let ip: [u8; IPV6_HEADER_LEN] = read(ctx, ETH_HEADER_LEN)?;
    // Extension headers, fragments among them, are not relay traffic.
    if ip[0] >> 4 != 6 || ip[6] != IPPROTO_UDP {
        return None;
    }
    let udp_offset = ETH_HEADER_LEN + IPV6_HEADER_LEN;
    let udp: [u8; UDP_HEADER_LEN] = read(ctx, udp_offset)?;
    if u16::from_be_bytes([udp[2], udp[3]]) != config.relay_port {
        return None;
    }
    // Every read before any length field is compared with the packet: see `read`.
    let head: Option<[u8; HEADER_LEN]> = read(ctx, udp_offset + UDP_HEADER_LEN);
    let segment_len = usize::from(u16::from_be_bytes([ip[4], ip[5]]));
    if segment_len < UDP_HEADER_LEN || !fits(ctx, ETH_HEADER_LEN + IPV6_HEADER_LEN + segment_len) {
        return None;
    }

    let mut source = [0u8; 16];
    source.copy_from_slice(&ip[8..24]);
    let payload_len = udp_payload_len(&udp, segment_len, false);
    let header = relay_header(head, payload_len);
    // SAFETY: copied out at once; see `ipv4`.
    let blocked = unsafe { BLOCK_V6.get(&source) }.copied();
    // SAFETY: copied out at once; see `ipv4`.
    let bucket = unsafe { RATE_V6.get(&source) }.copied();
    let judgement = judge(
        blocked,
        bucket,
        config.limit,
        now_ns(),
        header.as_ref(),
        payload_len.unwrap_or(0),
    );
    if let Some(next) = judgement.bucket {
        let _ = RATE_V6.insert(&source, &next, 0);
    }
    Some(act(ctx, judgement.verdict))
}

/// The UDP payload length, or `None` for a fragment or a length field that
/// disagrees with the IP layer — both malformed once addressed to the relay.
fn udp_payload_len(udp: &[u8; UDP_HEADER_LEN], segment_len: usize, is_fragment: bool) -> Option<usize> {
    if is_fragment {
        return None;
    }
    let udp_len = usize::from(u16::from_be_bytes([udp[4], udp[5]]));
    if udp_len < UDP_HEADER_LEN || udp_len > segment_len {
        return None;
    }
    Some(udp_len - UDP_HEADER_LEN)
}

/// The relay header, if the UDP payload is long enough to hold one. `head` was
/// read from the packet already; the UDP length, not the frame, decides.
fn relay_header(head: Option<[u8; HEADER_LEN]>, payload_len: Option<usize>) -> Option<[u8; HEADER_LEN]> {
    if payload_len? < HEADER_LEN {
        return None;
    }
    head
}

fn act(ctx: &XdpContext, verdict: Verdict) -> u32 {
    match verdict {
        // The low bits of the flags are the action if no socket is bound on
        // this queue: drop, never pass, so relay traffic cannot reach the
        // kernel stack by arriving on a queue nobody serves.
        Verdict::Redirect => match XSKS.redirect(ctx.rx_queue_index(), u64::from(xdp_action::XDP_DROP)) {
            Ok(action) => {
                count(Counter::Redirected);
                action
            }
            Err(action) => {
                count(Counter::RedirectFailed);
                action
            }
        },
        Verdict::Drop(counter) => {
            count(counter);
            xdp_action::XDP_DROP
        }
    }
}

/// Copies a `T` from `offset` into the packet, or `None` past its end.
///
/// # Why every read comes before every length comparison
///
/// The verifier accepts a packet read only if a comparison against `data_end`
/// on that same pointer precedes it. LLVM does not know that. Given an earlier
/// check such as "the IP total length fits the packet" that *implies* a later
/// read is in bounds, it deletes the later read's own check as redundant — and
/// the verifier then refuses the program ("invalid access to packet"), because
/// it cannot follow the implication through a scalar. That happened, on the
/// first load against a real kernel. So each function reads everything it
/// needs first, and only then compares header fields with the packet length:
/// a later check cannot make an earlier one redundant.
#[inline(always)]
fn read<T: Copy>(ctx: &XdpContext, offset: usize) -> Option<T> {
    let start = ctx.data();
    if start + offset + size_of::<T>() > ctx.data_end() {
        return None;
    }
    // SAFETY: `[start + offset, start + offset + size_of::<T>())` lies inside
    // the packet, checked against `data_end` above — the bound the verifier
    // enforces. Unaligned, because packet bytes have no alignment.
    Some(unsafe { ((start + offset) as *const T).read_unaligned() })
}

/// Whether the first `len` bytes of the frame exist. A semantic check on a
/// length field, never a guard for a read: see `read`.
#[inline(always)]
fn fits(ctx: &XdpContext, len: usize) -> bool {
    ctx.data() + len <= ctx.data_end()
}

#[inline(always)]
fn now_ns() -> u64 {
    // SAFETY: `bpf_ktime_get_ns` takes no arguments and reads CLOCK_MONOTONIC.
    unsafe { bpf_ktime_get_ns() }
}

fn count(counter: Counter) {
    if let Some(slot) = COUNTERS.get_ptr_mut(counter.slot()) {
        // SAFETY: a per-CPU slot from a successful lookup; XDP programs do not
        // preempt one another on a CPU, so nothing writes it concurrently.
        unsafe { *slot = (*slot).wrapping_add(1) };
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    // Unreachable in a verified program: the verifier refuses any path to it.
    loop {}
}
