//! Handshake cost measurement: bytes on the wire, segments, and wall clock.
//!
//! **Test and benchmark infrastructure.** It lives in the library rather than
//! in `tests/` for the same reason [`crate::network::sim`] does: an integration
//! test cannot reach a `#[cfg(test)]` item. Nothing on a production path calls
//! any of it.
//!
//! # Why this measures rather than asserts
//!
//! The dual-KEM work was scoped against a sub-100 ms connection target. A test
//! asserting that threshold would be worse than no test:
//!
//! - On loopback or an in-process duplex, **both** handshakes finish in well
//!   under a millisecond. The assertion would pass forever, including after a
//!   change that tripled the cost, and would report a green tick for a
//!   property it never examined.
//! - On a real link the number is dominated by round trips, which are a
//!   property of the path and not of this code. A CI machine's loopback cannot
//!   observe it, and a threshold tuned until CI passed would be measuring the
//!   CI runner.
//!
//! So this reports. [`Measurement`] carries the numbers; the caller decides
//! what to make of them, and `tests/dualkem_latency_tests.rs` prints a table.
//!
//! # The number that actually matters, and the trap in it
//!
//! Segment count — but of the **largest single message**, not of the handshake
//! total. Congestion control is per-direction, so the sum of both directions is
//! the wrong quantity, and dividing it by the MSS gives an answer that is wrong
//! in the alarming direction.
//!
//! | | Wire total | Largest message | Segments | Fits window? |
//! |---|---|---|---|---|
//! | `/maya/mlkem/1.0.0` | 2,274 | 1,185 | 1 | yes |
//! | `/maya/dualkem/1.0.0` | 15,766 | 5,699 | 4 | yes |
//!
//! The initial congestion window is [`INITIAL_CWND_SEGMENTS`] segments
//! (RFC 6928). **Both handshakes fit**, so neither pays an extra round trip.
//!
//! This is worth stating loudly because the design discussion assumed the
//! opposite: 15,766 divided by 1,460 is eleven, which exceeds ten, and that
//! arithmetic is easy to do and wrong. [`Measurement::exceeds_initial_window`]
//! takes the largest message precisely so the mistake is hard to repeat.
//!
//! The extra bytes are still real. They cost bandwidth on every connection, and
//! on a constrained or metered link that is the cost worth counting. They do
//! not cost latency.

use std::time::{Duration, Instant};

use futures::{AsyncRead, AsyncWrite};

use crate::error::NodeError;
use crate::network::pq::{dual, handshake};

/// Bytes in a TCP segment on a conventional 1500-byte-MTU path.
///
/// 1500 minus 20 bytes of IPv4 header and 20 of TCP header. IPv6 or any
/// tunnelling lowers it, which makes the segment counts here optimistic rather
/// than conservative.
pub const MSS_BYTES: usize = 1460;

/// Segments a sender may put in flight before the first ACK.
///
/// Ten, per RFC 6928, which is what Linux and every other mainstream stack has
/// defaulted to for over a decade. A handshake fitting inside this window costs
/// one round trip; one exceeding it costs two.
pub const INITIAL_CWND_SEGMENTS: usize = 10;

/// What one handshake cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Measurement {
    /// Protocol name, as negotiated.
    pub protocol: &'static str,
    /// Total bytes both directions put on the wire.
    pub wire_bytes: usize,
    /// Wall-clock time for the exchange, including simulated link delay.
    pub elapsed: Duration,
    /// One-way delay the simulated link applied to each read.
    pub link_delay: Duration,
}

impl Measurement {
    /// Segments the larger of the two directions occupies.
    ///
    /// The *larger* direction, not the total: congestion control is
    /// per-direction, and it is the responder's 5,699-byte message that has to
    /// fit in a window, not the sum of both sides.
    #[must_use]
    pub const fn segments(self, largest_message: usize) -> usize {
        largest_message.div_ceil(MSS_BYTES)
    }

    /// Whether the largest single message spills past the initial congestion
    /// window, and therefore costs an extra round trip on a cold connection.
    ///
    /// This is the finding the measurement exists to produce.
    #[must_use]
    pub const fn exceeds_initial_window(largest_message: usize) -> bool {
        largest_message.div_ceil(MSS_BYTES) > INITIAL_CWND_SEGMENTS
    }
}

/// Runs one single-KEM handshake over the given stream pair and reports its
/// cost.
///
/// The streams are supplied by the caller rather than built here: wrapping
/// them in [`crate::network::sim::DelayStream`] is how a link delay is
/// simulated, and the in-memory duplex they wrap is a dev-dependency the
/// library cannot reach. `link_delay` is recorded on the result, not applied —
/// applying it is the stream's job.
///
/// # Errors
///
/// Propagates any handshake failure. A measurement of a failed handshake would
/// be a number with no meaning, so it is not returned.
pub async fn measure_single<A, B>(
    mut a: A,
    mut b: B,
    link_delay: Duration,
) -> Result<Measurement, NodeError>
where
    A: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let started = Instant::now();
    let responder = tokio::spawn(async move { handshake::respond(&mut a).await });
    let initiator = handshake::initiate(&mut b).await?;
    let responder = responder
        .await
        .map_err(|e| NodeError::PqHandshake(format!("responder task failed: {e}")))??;
    let elapsed = started.elapsed();

    agree(
        initiator.initiator_to_responder.as_slice(),
        responder.initiator_to_responder.as_slice(),
    )?;

    Ok(Measurement {
        protocol: crate::network::pq::PROTOCOL,
        wire_bytes: handshake::RESPONDER_MESSAGE_LEN + handshake::INITIATOR_MESSAGE_LEN,
        elapsed,
        link_delay,
    })
}

/// Runs one dual-KEM handshake over the given stream pair and reports its cost.
///
/// # Errors
///
/// Propagates any handshake failure, for the same reason as
/// [`measure_single`].
pub async fn measure_dual<A, B>(
    mut a: A,
    mut b: B,
    link_delay: Duration,
) -> Result<Measurement, NodeError>
where
    A: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let started = Instant::now();
    let responder = tokio::spawn(async move { dual::respond(&mut a).await });
    let initiator = dual::initiate(&mut b).await?;
    let responder = responder
        .await
        .map_err(|e| NodeError::PqHandshake(format!("responder task failed: {e}")))??;
    let elapsed = started.elapsed();

    agree(
        initiator.initiator_to_responder.as_slice(),
        responder.initiator_to_responder.as_slice(),
    )?;

    Ok(Measurement {
        protocol: dual::PROTOCOL,
        wire_bytes: dual::HANDSHAKE_BYTES,
        elapsed,
        link_delay,
    })
}

/// Confirms both sides derived the same key.
///
/// A handshake that completed without agreeing is not a fast handshake, it is
/// a broken one, and timing it would report a number for something that does
/// not work.
fn agree(left: &[u8], right: &[u8]) -> Result<(), NodeError> {
    if left == right {
        Ok(())
    } else {
        Err(NodeError::PqHandshake(
            "handshake completed but the two sides disagree on the session key".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_single_kem_handshake_fits_the_initial_window() {
        assert!(!Measurement::exceeds_initial_window(
            handshake::RESPONDER_MESSAGE_LEN
        ));
    }

    #[test]
    fn the_dual_kem_responder_message_also_fits_the_initial_window() {
        // The correction, pinned. An early draft assumed the dual handshake
        // spilled past the window and cost a round trip; it does not, because
        // the quantity that matters is the largest single message and not the
        // two-direction total. If a future parameter set grows past ten
        // segments this fails, and the round-trip cost becomes real.
        assert!(!Measurement::exceeds_initial_window(
            dual::RESPONDER_MESSAGE_LEN
        ));
        assert_eq!(dual::RESPONDER_MESSAGE_LEN.div_ceil(MSS_BYTES), 4);
    }

    #[test]
    fn the_wire_sizes_are_what_the_documentation_quotes() {
        assert_eq!(
            handshake::RESPONDER_MESSAGE_LEN + handshake::INITIATOR_MESSAGE_LEN,
            2_274
        );
        assert_eq!(dual::HANDSHAKE_BYTES, 15_766);
    }
}
