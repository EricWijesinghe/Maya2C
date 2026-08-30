//! Network simulation support: a transport that injects latency.
//!
//! **This is test and benchmark infrastructure.** It lives in the library
//! rather than in `tests/` because an integration test cannot reach a
//! `#[cfg(test)]` item, and building a node with a modified transport requires
//! a constructor the library exports. Nothing on a production path constructs a
//! [`DelayTransport`], and [`crate::network::Node::new_memory`] is unaffected.
//!
//! ## What it is for
//!
//! Every default test in this repository runs on libp2p's in-process memory
//! transport, where a write is a `Vec` push and latency is effectively zero.
//! That is the right default — it makes the tests fast and free of OS
//! flakiness — but it means the whole suite validates the protocol under
//! conditions no real network provides.
//!
//! It matters more now than it used to. The post-quantum upgrade adds a
//! two-message round trip to every connection ([`crate::network::pq`]), and a
//! round trip is exactly the thing latency multiplies. On a zero-latency
//! transport that cost is invisible; at 200 ms RTT it is the dominant term in
//! connection setup. A simulation that never delays anything cannot tell the
//! difference between "the handshake works" and "the handshake works when it
//! costs nothing".
//!
//! ## What it models, and what it does not
//!
//! It delays reads. Each `poll_read` waits before it resolves, which
//! approximates one-way propagation delay well enough to expose round-trip
//! sensitivity and to order events realistically.
//!
//! It does not model bandwidth, jitter, packet loss, reordering, or queueing —
//! all of which a real network has and some of which change protocol behaviour.
//! A passing run here is evidence that latency alone does not break
//! propagation. It is not evidence that the protocol survives a bad network,
//! and the tests that use it say so.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use std::time::Duration;

use futures::{AsyncRead, AsyncWrite};

/// A stream whose reads resolve only after a fixed delay.
///
/// Wraps any connection the memory transport produces.
pub struct DelayStream<S> {
    inner: S,
    delay: Duration,
    /// The timer for the read currently in flight, if one is armed.
    ///
    /// Boxed because `tokio::time::Sleep` is `!Unpin` and this struct has to be
    /// `Unpin` for libp2p's upgrade machinery to accept it.
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl<S> DelayStream<S> {
    /// Wraps `inner`, delaying every read by `delay`.
    pub fn new(inner: S, delay: Duration) -> Self {
        Self {
            inner,
            delay,
            sleep: None,
        }
    }
}

impl<S> AsyncRead for DelayStream<S>
where
    S: AsyncRead + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        // Zero delay short-circuits entirely rather than arming a timer that
        // fires immediately. A zero-duration sleep still costs a trip through
        // the timer wheel, and the 0 ms baseline case is the one every other
        // measurement is compared against.
        if this.delay.is_zero() {
            return Pin::new(&mut this.inner).poll_read(cx, buf);
        }

        let sleep = this
            .sleep
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(this.delay)));
        ready!(sleep.as_mut().poll(cx));

        // Disarmed before the read, not after: the read may itself return
        // `Pending`, and leaving an elapsed timer in place would make the next
        // poll wait a second time for a delay already served.
        this.sleep = None;
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl<S> AsyncWrite for DelayStream<S>
where
    S: AsyncWrite + Unpin,
{
    /// Writes are not delayed.
    ///
    /// Delaying both directions would double every one-way figure and make the
    /// numbers in the tests mean something other than what they say. The peer's
    /// read is where its delay is applied.
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
}

/// Round-trip latencies the simulation sweeps.
///
/// Chosen to bracket what a real deployment sees rather than to be round
/// numbers: 0 is the control, 50 ms is a continental hop, 200 ms is
/// intercontinental, and 500 ms is a satellite link or a badly congested path —
/// past the point where most peer-to-peer software has already given up.
pub const LATENCY_SWEEP: [Duration; 4] = [
    Duration::ZERO,
    Duration::from_millis(25),
    Duration::from_millis(100),
    Duration::from_millis(250),
];

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{AsyncReadExt, AsyncWriteExt};
    use std::time::Instant;

    #[tokio::test]
    async fn a_read_waits_for_the_delay() {
        let (mut a, b) = futures_ringbuf::Endpoint::pair(256, 256);
        let mut delayed = DelayStream::new(b, Duration::from_millis(80));

        a.write_all(b"hello").await.expect("write");
        a.flush().await.expect("flush");

        let started = Instant::now();
        let mut buf = [0u8; 5];
        delayed.read_exact(&mut buf).await.expect("read");
        let elapsed = started.elapsed();

        assert_eq!(&buf, b"hello");
        assert!(
            elapsed >= Duration::from_millis(70),
            "read returned in {elapsed:?}, faster than the injected delay"
        );
    }

    #[tokio::test]
    async fn a_zero_delay_stream_does_not_wait() {
        let (mut a, b) = futures_ringbuf::Endpoint::pair(256, 256);
        let mut delayed = DelayStream::new(b, Duration::ZERO);

        a.write_all(b"hello").await.expect("write");
        a.flush().await.expect("flush");

        let started = Instant::now();
        let mut buf = [0u8; 5];
        delayed.read_exact(&mut buf).await.expect("read");

        assert_eq!(&buf, b"hello");
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn writes_are_not_delayed() {
        let (a, mut b) = futures_ringbuf::Endpoint::pair(256, 256);
        let mut delayed = DelayStream::new(a, Duration::from_millis(200));

        let started = Instant::now();
        delayed.write_all(b"hello").await.expect("write");
        delayed.flush().await.expect("flush");
        assert!(started.elapsed() < Duration::from_millis(100));

        let mut buf = [0u8; 5];
        b.read_exact(&mut buf).await.expect("read");
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn the_sweep_starts_from_a_zero_control() {
        // Every latency figure the simulation reports is read against the first
        // entry, so it has to be the no-delay case.
        assert_eq!(LATENCY_SWEEP[0], Duration::ZERO);
        assert!(LATENCY_SWEEP.windows(2).all(|w| w[0] < w[1]));
    }
}
