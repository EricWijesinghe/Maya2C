//! A block frame's claimed transaction count does not decide how much the
//! decoder reserves (PR #702 review). The count bound is the byte ceiling at
//! nine bytes an entry (815,559), so one unauthenticated 7.3 MiB frame of
//! 1-byte entries could otherwise reserve 815,559 transactions' worth of
//! memory before its first entry failed to decode.
//!
//! One test in its own binary: the allocator below is process-wide.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use custom_l1_node::core::block::{Block, BlockHeader, HEADER_LEN};
use custom_l1_node::core::transaction::Transaction;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use maya_fee_market::limits::BLOCK_TXS_CEILING;

struct Largest;

static WATCHING: AtomicBool = AtomicBool::new(false);
static LARGEST: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards every call to `System` unchanged; it only records sizes.
unsafe impl GlobalAlloc for Largest {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if WATCHING.load(Ordering::Relaxed) {
            LARGEST.fetch_max(layout.size(), Ordering::Relaxed);
        }
        // SAFETY: the caller's contract for `alloc` is passed through as is.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` came from `System.alloc` with this `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Largest = Largest;

#[test]
fn a_frame_claiming_the_most_entries_reserves_a_bounded_vector() {
    let header = BlockHeader {
        version: 0,
        prev_hash: [0; 32],
        state_root: [0; 32],
        timestamp: 1_790_000_001,
        nonce: 0,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [0; 32],
    };
    let mut frame = Block::new(header, vec![]).to_bytes();
    frame.truncate(HEADER_LEN);
    frame.extend_from_slice(&BLOCK_TXS_CEILING.to_le_bytes());
    // Each entry: a length of 1, then one byte no transaction decodes from.
    for _ in 0..BLOCK_TXS_CEILING {
        frame.extend_from_slice(&1u64.to_le_bytes());
        frame.push(0xff);
    }

    WATCHING.store(true, Ordering::Relaxed);
    let decoded = Block::from_bytes(&frame);
    WATCHING.store(false, Ordering::Relaxed);

    assert!(decoded.is_err(), "a 1-byte entry is not a transaction");
    let claimed = usize::try_from(BLOCK_TXS_CEILING).unwrap() * size_of::<Transaction>();
    let largest = LARGEST.load(Ordering::Relaxed);
    assert!(
        largest * 64 < claimed,
        "reserved {largest} bytes up front for a frame claiming {claimed}"
    );
}
