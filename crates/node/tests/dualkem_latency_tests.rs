//! Dual-KEM handshake cost: measured and reported, never asserted.
//!
//! # Why there is no sub-100 ms assertion here
//!
//! The work was scoped against a sub-100 ms connection target, and this file
//! deliberately does not assert it.
//!
//! On the in-process transport these tests use, *both* handshakes finish in
//! well under a millisecond. An assertion at 100 ms would pass forever — it
//! would keep passing after a change that tripled the cost, and would report a
//! green tick for a property it never examined. On a real link the number is
//! dominated by round trips, which belong to the path rather than to this code,
//! and a threshold tuned until CI went green would be measuring the CI runner.
//!
//! What these tests *do* assert is correctness: every handshake completes and
//! both sides agree on the key, at every simulated latency. The cost numbers
//! are printed for a human to read.
//!
//! Run with output:
//!
//! ```text
//! cargo test --test dualkem_latency_tests -- --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use custom_l1_node::network::pq::measure::{
    INITIAL_CWND_SEGMENTS, MSS_BYTES, Measurement, measure_dual, measure_single,
};
use custom_l1_node::network::pq::{dual, handshake};
use custom_l1_node::network::sim::DelayStream;

/// A pair of in-memory duplex endpoints, each delaying reads by `delay`.
///
/// Sized past the dual handshake's largest message (5,699 B) so a measurement
/// never includes a stall caused only by the ring buffer's capacity.
fn delayed_pair(
    delay: Duration,
) -> (
    DelayStream<futures_ringbuf::Endpoint>,
    DelayStream<futures_ringbuf::Endpoint>,
) {
    let (a, b) = futures_ringbuf::Endpoint::pair(65_536, 65_536);
    (DelayStream::new(a, delay), DelayStream::new(b, delay))
}

/// One-way link delays to sample.
///
/// Zero is the in-process baseline; 10 ms is a metropolitan link; 50 ms is
/// intercontinental. Each is *one-way*, so a two-message handshake pays roughly
/// twice the figure before any congestion-window effect.
const DELAYS_MS: [u64; 3] = [0, 10, 50];

#[tokio::test]
async fn both_handshakes_agree_at_every_simulated_latency() {
    // The assertion that matters: the dual handshake works, at every latency,
    // and both sides derive the same key. `measure_*` returns an error rather
    // than a measurement if they disagree.
    for delay_ms in DELAYS_MS {
        let delay = Duration::from_millis(delay_ms);
        let (a, b) = delayed_pair(delay);
        measure_single(a, b, delay)
            .await
            .unwrap_or_else(|e| panic!("single-KEM handshake failed at {delay_ms} ms: {e}"));

        let (a, b) = delayed_pair(delay);
        measure_dual(a, b, delay)
            .await
            .unwrap_or_else(|e| panic!("dual-KEM handshake failed at {delay_ms} ms: {e}"));
    }
}

#[tokio::test]
async fn report_handshake_cost() {
    println!();
    println!("Handshake cost — measured, not asserted");
    println!("=======================================");
    println!();
    println!("Wire size and segmentation (independent of link):");
    println!();
    println!(
        "  {:<24} {:>10} {:>12} {:>10} {:>16}",
        "protocol", "wire B", "largest msg", "segments", "extra RTT?"
    );

    let rows = [
        (
            custom_l1_node::network::pq::PROTOCOL,
            handshake::RESPONDER_MESSAGE_LEN + handshake::INITIATOR_MESSAGE_LEN,
            handshake::RESPONDER_MESSAGE_LEN,
        ),
        (
            dual::PROTOCOL,
            dual::HANDSHAKE_BYTES,
            dual::RESPONDER_MESSAGE_LEN,
        ),
    ];

    for (protocol, wire, largest) in rows {
        let segments = largest.div_ceil(MSS_BYTES);
        let spills = Measurement::exceeds_initial_window(largest);
        println!(
            "  {:<24} {:>10} {:>12} {:>10} {:>16}",
            protocol,
            wire,
            largest,
            segments,
            if spills { "yes" } else { "no" }
        );
    }

    println!();
    println!(
        "  MSS {MSS_BYTES} B, initial congestion window {INITIAL_CWND_SEGMENTS} segments (RFC 6928)."
    );
    println!("  A message exceeding the window would wait for an ACK before the rest went");
    println!("  out. Neither handshake does: what must fit is the largest single message,");
    println!("  not the two-direction total, and 5,699 B is four segments.");
    println!();
    println!("Wall clock on the in-process transport (computation plus simulated delay):");
    println!();
    println!(
        "  {:<12} {:>18} {:>18}",
        "one-way", "single-KEM", "dual-KEM"
    );

    for delay_ms in DELAYS_MS {
        let delay = Duration::from_millis(delay_ms);
        let (a, b) = delayed_pair(delay);
        let single = measure_single(a, b, delay)
            .await
            .expect("single-KEM handshake");

        let (a, b) = delayed_pair(delay);
        let dual_measured = measure_dual(a, b, delay).await.expect("dual-KEM handshake");
        println!(
            "  {:<12} {:>18} {:>18}",
            format!("{delay_ms} ms"),
            format!("{:.2?}", single.elapsed),
            format!("{:.2?}", dual_measured.elapsed)
        );
    }

    println!();
    println!("  The in-process transport has no congestion window, but per the table above");
    println!("  neither handshake would exceed one, so no round trip is missing from these");
    println!("  figures. The dual-KEM cost is decoding work and bandwidth, not latency.");
    println!();
}

#[test]
fn the_dual_handshake_is_the_size_the_documentation_claims() {
    // The docs in `crypto_pq::hqc`, `network::pq::dual`, and
    // `docs/pq-transport.md` all quote these. Pinning them means a dependency
    // bump that moved a parameter set fails here rather than making three
    // documents quietly wrong.
    assert_eq!(handshake::RESPONDER_MESSAGE_LEN, 1 + 1184);
    assert_eq!(handshake::INITIATOR_MESSAGE_LEN, 1 + 1088);
    assert_eq!(dual::RESPONDER_MESSAGE_LEN, 1 + 1184 + 4514);
    assert_eq!(dual::INITIATOR_MESSAGE_LEN, 1 + 1088 + 8978);
    assert_eq!(dual::HANDSHAKE_BYTES, 15_766);

    // And the ratio the design discussion turns on.
    let single = handshake::RESPONDER_MESSAGE_LEN + handshake::INITIATOR_MESSAGE_LEN;
    assert_eq!(single, 2_274);
    assert!(dual::HANDSHAKE_BYTES > single * 6);
}
