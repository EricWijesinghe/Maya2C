//! The encrypted duplex that sits above the Noise session.
//!
//! Once [`super::handshake`] has produced directional keys, every byte the node
//! sends is framed and sealed with ChaCha20-Poly1305 under a key an adversary
//! must break ML-KEM-768 to derive. The Noise layer underneath keeps doing its
//! job; this adds a second, post-quantum one on top.
//!
//! ## Framing
//!
//! ```text
//! [4 bytes big-endian length][ciphertext .. length bytes, tag included]
//! ```
//!
//! Fixed 4-byte prefix rather than a varint: a varint saves three bytes on a
//! frame that is already up to 64 KiB, and costs an incremental decoder that
//! has to handle a hostile peer stretching a length across many reads.
//!
//! The length covers the ciphertext *and* its 16-byte tag, so a frame is
//! self-delimiting and a truncated one fails to authenticate rather than being
//! silently accepted short.
//!
//! ## Nonces
//!
//! A 64-bit counter per direction, little-endian in the low 8 bytes of the
//! 12-byte nonce, starting at zero and never reused. Two properties matter:
//!
//! - **Separate counters per direction.** The two directions use different keys
//!   ([`super::handshake::SessionKeys`]), so even identical counters are safe;
//!   separate counters mean the two facts are independent rather than one
//!   propping up the other.
//! - **No wraparound, ever.** At `u64::MAX` this returns an error and the
//!   connection dies. Reaching it would take longer than the universe has
//!   existed at any plausible frame rate, but a counter that silently wrapped
//!   would repeat a `(key, nonce)` pair, and repeating one destroys
//!   ChaCha20-Poly1305's security outright. A hard failure is the only correct
//!   behaviour and it costs one comparison per frame.
//!
//! ## Hostile input
//!
//! Every length is checked against [`MAX_FRAME_LEN`] before a single byte is
//! allocated, mirroring the discipline in [`crate::core::codec`]. A peer that
//! announces a 4 GiB frame gets an error, not an allocation.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use futures::{AsyncRead, AsyncWrite};

use super::handshake::SessionKeys;

/// Largest plaintext carried in one frame.
///
/// 64 KiB. Large enough that a multi-megabyte block costs tens of frames rather
/// than thousands; small enough that a peer cannot make us buffer much before
/// the tag proves the frame genuine.
pub const MAX_PLAINTEXT_LEN: usize = 64 * 1024;

/// Poly1305 authentication tag length.
const TAG_LEN: usize = 16;

/// Largest ciphertext-plus-tag a peer may announce.
pub const MAX_FRAME_LEN: usize = MAX_PLAINTEXT_LEN + TAG_LEN;

/// Bytes of length prefix.
const LENGTH_PREFIX_LEN: usize = 4;

/// One direction's cipher and nonce counter.
struct Direction {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl Direction {
    fn new(key: &[u8; 32]) -> Self {
        Self {
            // Infallible for a 32-byte key, which is the only thing that
            // reaches here: `SessionKeys` holds fixed-width arrays.
            cipher: ChaCha20Poly1305::new(key.into()),
            counter: 0,
        }
    }

    /// The next nonce, refusing to wrap.
    fn next_nonce(&mut self) -> io::Result<Nonce> {
        let counter = self.counter;
        self.counter = counter.checked_add(1).ok_or_else(|| {
            io::Error::other("ML-KEM session nonce space exhausted; refusing to reuse a nonce")
        })?;

        let mut bytes = [0u8; 12];
        bytes[4..].copy_from_slice(&counter.to_le_bytes());
        Ok(Nonce::from(bytes))
    }

    fn seal(&mut self, buffer: &mut Vec<u8>) -> io::Result<()> {
        let nonce = self.next_nonce()?;
        self.cipher
            .encrypt_in_place(&nonce, b"", buffer)
            .map_err(|_| io::Error::other("ML-KEM session encryption failed"))
    }

    fn open(&mut self, buffer: &mut Vec<u8>) -> io::Result<()> {
        let nonce = self.next_nonce()?;
        self.cipher
            .decrypt_in_place(&nonce, b"", buffer)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "ML-KEM session frame failed authentication",
                )
            })
    }
}

/// What the read half is currently doing.
enum ReadState {
    /// Accumulating the 4-byte length prefix.
    Length { filled: usize },
    /// Accumulating `needed` bytes of frame body.
    Body { needed: usize },
    /// Holding decrypted bytes not yet handed to the caller.
    Plaintext { position: usize },
}

/// A stream whose every byte is sealed under a post-quantum session key.
///
/// Wraps the Noise-authenticated connection libp2p hands us and is itself
/// handed to yamux, so the multiplexer and everything above it — gossipsub
/// included — see only this.
pub struct PqStream<S> {
    inner: S,
    send: Direction,
    recv: Direction,

    read_state: ReadState,
    /// Serves as the length prefix while reading a header, then the frame body,
    /// then the decrypted plaintext. One buffer because the three phases never
    /// overlap and a frame is decrypted in place.
    read_buf: Vec<u8>,
    length_prefix: [u8; LENGTH_PREFIX_LEN],

    /// An encoded frame waiting to reach the wire, and how much has gone.
    write_buf: Vec<u8>,
    write_position: usize,
}

impl<S> PqStream<S> {
    /// Wraps `inner` with keys from a completed handshake.
    ///
    /// `is_initiator` selects which directional key seals and which opens —
    /// getting it backwards produces a connection where neither side can read
    /// the other, which the round-trip tests catch immediately.
    pub fn new(inner: S, keys: SessionKeys, is_initiator: bool) -> Self {
        let (send_key, recv_key) = if is_initiator {
            (&keys.initiator_to_responder, &keys.responder_to_initiator)
        } else {
            (&keys.responder_to_initiator, &keys.initiator_to_responder)
        };

        Self {
            inner,
            send: Direction::new(send_key),
            recv: Direction::new(recv_key),
            read_state: ReadState::Length { filled: 0 },
            read_buf: Vec::new(),
            length_prefix: [0u8; LENGTH_PREFIX_LEN],
            write_buf: Vec::new(),
            write_position: 0,
        }
    }
}

impl<S> PqStream<S>
where
    S: AsyncWrite + Unpin,
{
    /// Pushes whatever of `write_buf` still has not reached the wire.
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.write_position < self.write_buf.len() {
            let written = ready!(
                Pin::new(&mut self.inner).poll_write(cx, &self.write_buf[self.write_position..])
            )?;
            if written == 0 {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "peer accepted no bytes",
                )));
            }
            self.write_position += written;
        }

        self.write_buf.clear();
        self.write_position = 0;
        Poll::Ready(Ok(()))
    }
}

impl<S> AsyncRead for PqStream<S>
where
    S: AsyncRead + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        loop {
            match this.read_state {
                ReadState::Plaintext { position } => {
                    // An empty frame is legal on the wire but carries nothing,
                    // so serving it as a zero-length read would look like EOF.
                    // Fall through to the next frame instead.
                    if position < this.read_buf.len() {
                        let take = (this.read_buf.len() - position).min(out.len());
                        out[..take].copy_from_slice(&this.read_buf[position..position + take]);

                        let position = position + take;
                        if position == this.read_buf.len() {
                            this.read_buf.clear();
                            this.read_state = ReadState::Length { filled: 0 };
                        } else {
                            this.read_state = ReadState::Plaintext { position };
                        }
                        return Poll::Ready(Ok(take));
                    }

                    this.read_buf.clear();
                    this.read_state = ReadState::Length { filled: 0 };
                }

                ReadState::Length { mut filled } => {
                    while filled < LENGTH_PREFIX_LEN {
                        let read = ready!(
                            Pin::new(&mut this.inner)
                                .poll_read(cx, &mut this.length_prefix[filled..])
                        )?;
                        if read == 0 {
                            // Clean EOF only on a frame boundary. A peer that
                            // vanished mid-prefix truncated the stream, and
                            // reporting that as EOF would hide it.
                            return if filled == 0 {
                                Poll::Ready(Ok(0))
                            } else {
                                Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    "stream ended inside a frame length",
                                )))
                            };
                        }
                        filled += read;
                        this.read_state = ReadState::Length { filled };
                    }

                    let length = u32::from_be_bytes(this.length_prefix) as usize;

                    // Checked before allocating. A peer announcing a 4 GiB
                    // frame gets an error, not an out-of-memory abort.
                    if length > MAX_FRAME_LEN {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("frame of {length} bytes exceeds the {MAX_FRAME_LEN} maximum"),
                        )));
                    }
                    if length < TAG_LEN {
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("frame of {length} bytes cannot hold a {TAG_LEN}-byte tag"),
                        )));
                    }

                    this.read_buf.clear();
                    this.read_buf.resize(length, 0);
                    this.read_state = ReadState::Body { needed: length };
                }

                ReadState::Body { mut needed } => {
                    while needed > 0 {
                        let offset = this.read_buf.len() - needed;
                        let read = ready!(
                            Pin::new(&mut this.inner).poll_read(cx, &mut this.read_buf[offset..])
                        )?;
                        if read == 0 {
                            return Poll::Ready(Err(io::Error::new(
                                io::ErrorKind::UnexpectedEof,
                                "stream ended inside a frame body",
                            )));
                        }
                        needed -= read;
                        this.read_state = ReadState::Body { needed };
                    }

                    // Decrypts in place; `read_buf` shrinks by the tag.
                    this.recv.open(&mut this.read_buf)?;
                    this.read_state = ReadState::Plaintext { position: 0 };
                }
            }
        }
    }
}

impl<S> AsyncWrite for PqStream<S>
where
    S: AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();

        // One frame in flight at a time. Buffering a second before the first
        // has left would let a slow peer make us hold unbounded memory.
        ready!(this.poll_drain(cx))?;

        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let take = data.len().min(MAX_PLAINTEXT_LEN);

        let mut frame = Vec::with_capacity(LENGTH_PREFIX_LEN + take + TAG_LEN);
        frame.extend_from_slice(&[0u8; LENGTH_PREFIX_LEN]);
        frame.extend_from_slice(&data[..take]);

        // Seal the payload only. Splitting the buffer keeps the length prefix
        // outside the ciphertext, where the reader needs it in the clear.
        let mut payload = frame.split_off(LENGTH_PREFIX_LEN);
        this.send.seal(&mut payload)?;

        let length = u32::try_from(payload.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "sealed frame exceeds u32 length",
            )
        })?;
        frame[..LENGTH_PREFIX_LEN].copy_from_slice(&length.to_be_bytes());
        frame.append(&mut payload);

        this.write_buf = frame;
        this.write_position = 0;

        // Push what we can now, but report the write as accepted either way:
        // the bytes are ours and `poll_flush` will finish the job.
        let _ = this.poll_drain(cx)?;

        Poll::Ready(Ok(take))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_close(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::pq::handshake::{initiate, respond};
    use futures::{AsyncReadExt, AsyncWriteExt};

    /// A connected pair of `PqStream`s, handshake already run.
    async fn connected() -> (
        PqStream<futures_ringbuf::Endpoint>,
        PqStream<futures_ringbuf::Endpoint>,
    ) {
        let (mut a, mut b) = futures_ringbuf::Endpoint::pair(8192, 8192);
        let responder = tokio::spawn(async move {
            let keys = respond(&mut a).await.expect("respond");
            (a, keys)
        });
        let initiator_keys = initiate(&mut b).await.expect("initiate");
        let (a, responder_keys) = responder.await.expect("join");

        (
            PqStream::new(a, responder_keys, false),
            PqStream::new(b, initiator_keys, true),
        )
    }

    #[tokio::test]
    async fn a_message_round_trips() {
        let (mut responder, mut initiator) = connected().await;

        initiator.write_all(b"block 12345").await.expect("write");
        initiator.flush().await.expect("flush");

        let mut received = [0u8; 11];
        responder.read_exact(&mut received).await.expect("read");
        assert_eq!(&received, b"block 12345");
    }

    #[tokio::test]
    async fn both_directions_work_independently() {
        let (mut responder, mut initiator) = connected().await;

        initiator.write_all(b"ping").await.expect("write");
        initiator.flush().await.expect("flush");
        let mut buf = [0u8; 4];
        responder.read_exact(&mut buf).await.expect("read");
        assert_eq!(&buf, b"ping");

        responder.write_all(b"pong").await.expect("write");
        responder.flush().await.expect("flush");
        let mut buf = [0u8; 4];
        initiator.read_exact(&mut buf).await.expect("read");
        assert_eq!(&buf, b"pong");
    }

    #[tokio::test]
    async fn a_payload_larger_than_one_frame_survives() {
        // A block is megabytes; the framing has to split it and put it back
        // together without the caller noticing.
        let payload: Vec<u8> = (0..MAX_PLAINTEXT_LEN * 3 + 517)
            .map(|i| (i % 251) as u8)
            .collect();
        let expected = payload.clone();

        let (mut responder, mut initiator) = connected().await;

        let writer = tokio::spawn(async move {
            initiator.write_all(&payload).await.expect("write");
            initiator.flush().await.expect("flush");
            initiator
        });

        let mut received = vec![0u8; expected.len()];
        responder.read_exact(&mut received).await.expect("read");
        let _ = writer.await.expect("join");

        assert_eq!(received, expected);
    }

    #[tokio::test]
    async fn many_small_messages_keep_their_order_and_nonces() {
        // Exercises the nonce counter across many frames. A counter that reset
        // or repeated would surface here as an authentication failure.
        let (mut responder, mut initiator) = connected().await;

        let writer = tokio::spawn(async move {
            for index in 0u16..256 {
                initiator
                    .write_all(&index.to_be_bytes())
                    .await
                    .expect("write");
                initiator.flush().await.expect("flush");
            }
            initiator
        });

        for index in 0u16..256 {
            let mut buf = [0u8; 2];
            responder.read_exact(&mut buf).await.expect("read");
            assert_eq!(u16::from_be_bytes(buf), index);
        }
        let _ = writer.await.expect("join");
    }

    #[tokio::test]
    async fn the_bytes_on_the_wire_are_not_the_plaintext() {
        // The claim the whole module exists for, asserted rather than assumed.
        let (mut a, mut b) = futures_ringbuf::Endpoint::pair(8192, 8192);
        let responder = tokio::spawn(async move {
            let keys = respond(&mut a).await.expect("respond");
            (a, keys)
        });
        let keys = initiate(&mut b).await.expect("initiate");
        let (mut raw, _) = responder.await.expect("join");

        let mut initiator = PqStream::new(b, keys, true);
        initiator
            .write_all(b"SECRET-MARKER-maya2c")
            .await
            .expect("write");
        initiator.flush().await.expect("flush");

        let mut wire = [0u8; 64];
        let read = raw.read(&mut wire).await.expect("read raw");
        let seen = &wire[..read];

        assert!(
            !seen.windows(20).any(|w| w == b"SECRET-MARKER-maya2c"),
            "plaintext appeared on the wire"
        );
    }

    #[tokio::test]
    async fn a_tampered_frame_fails_authentication() {
        let (mut a, mut b) = futures_ringbuf::Endpoint::pair(8192, 8192);
        let responder = tokio::spawn(async move {
            let keys = respond(&mut a).await.expect("respond");
            (a, keys)
        });
        let initiator_keys = initiate(&mut b).await.expect("initiate");
        let (a, responder_keys) = responder.await.expect("join");

        let mut initiator = PqStream::new(b, initiator_keys, true);
        initiator.write_all(b"transfer").await.expect("write");
        initiator.flush().await.expect("flush");

        // Read the raw frame, flip a ciphertext byte, and feed it to a fresh
        // reader over a second pipe.
        let mut raw = a;
        let mut wire = vec![0u8; 128];
        let read = raw.read(&mut wire).await.expect("read raw");
        wire.truncate(read);
        let last = wire.len() - 1;
        wire[last] ^= 0x01;

        let (mut feed, sink) = futures_ringbuf::Endpoint::pair(8192, 8192);
        futures::AsyncWriteExt::write_all(&mut feed, &wire)
            .await
            .expect("feed");
        futures::AsyncWriteExt::flush(&mut feed)
            .await
            .expect("flush");

        let mut responder = PqStream::new(sink, responder_keys, false);
        let mut out = [0u8; 8];
        let error = responder.read_exact(&mut out).await.expect_err("must fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn an_oversized_length_prefix_is_refused_before_allocating() {
        let (mut feed, sink) = futures_ringbuf::Endpoint::pair(64, 64);
        futures::AsyncWriteExt::write_all(&mut feed, &u32::MAX.to_be_bytes())
            .await
            .expect("feed");
        futures::AsyncWriteExt::flush(&mut feed)
            .await
            .expect("flush");

        let keys = SessionKeys {
            initiator_to_responder: zeroize::Zeroizing::new([1u8; 32]),
            responder_to_initiator: zeroize::Zeroizing::new([2u8; 32]),
        };
        let mut stream = PqStream::new(sink, keys, false);

        let mut out = [0u8; 8];
        let error = stream.read(&mut out).await.expect_err("must refuse");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains("exceeds the"),
            "unhelpful error: {error}"
        );
    }

    #[tokio::test]
    async fn a_frame_too_short_to_hold_a_tag_is_refused() {
        let (mut feed, sink) = futures_ringbuf::Endpoint::pair(64, 64);
        futures::AsyncWriteExt::write_all(&mut feed, &3u32.to_be_bytes())
            .await
            .expect("feed");
        futures::AsyncWriteExt::flush(&mut feed)
            .await
            .expect("flush");

        let keys = SessionKeys {
            initiator_to_responder: zeroize::Zeroizing::new([1u8; 32]),
            responder_to_initiator: zeroize::Zeroizing::new([2u8; 32]),
        };
        let mut stream = PqStream::new(sink, keys, false);

        let mut out = [0u8; 8];
        let error = stream.read(&mut out).await.expect_err("must refuse");
        assert!(
            error.to_string().contains("cannot hold"),
            "unhelpful error: {error}"
        );
    }

    #[test]
    fn the_nonce_counter_refuses_to_wrap() {
        // Unreachable in practice and load-bearing anyway: a wrapped counter
        // repeats a (key, nonce) pair, which is total loss for ChaCha20.
        let mut direction = Direction::new(&[7u8; 32]);
        direction.counter = u64::MAX;

        let error = direction.next_nonce().expect_err("must refuse");
        assert!(
            error.to_string().contains("nonce space exhausted"),
            "unhelpful error: {error}"
        );
    }

    #[test]
    fn successive_nonces_differ() {
        let mut direction = Direction::new(&[7u8; 32]);
        let first = direction.next_nonce().expect("nonce");
        let second = direction.next_nonce().expect("nonce");
        assert_ne!(first, second);
    }
}
