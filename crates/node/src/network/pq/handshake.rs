//! The ML-KEM-768 key exchange, run inside an established Noise session.
//!
//! ## The exchange
//!
//! Two messages, both fixed-width, because ML-KEM has no variable-length
//! encodings and a length prefix would only add a field an attacker can lie
//! about:
//!
//! ```text
//! responder → initiator   [1 byte version][1184 byte encapsulation key]
//! initiator → responder   [1 byte version][1088 byte ciphertext]
//! ```
//!
//! The responder generates a keypair, sends the public half, and decapsulates
//! what comes back. The initiator encapsulates to it. Both then hold the same
//! 32-byte secret — or, if anything was tampered with, two different secrets
//! and a connection that fails on its first frame.
//!
//! ## Why this is safe to keep so small
//!
//! It is not an authenticated key exchange, and it does not try to be. It runs
//! *inside* a completed Noise XX session, which has already authenticated the
//! peer against its `PeerId` and encrypted the channel. An active attacker
//! cannot reach these bytes without first breaking X25519 in real time.
//!
//! So the job here is narrow: add a secret that a *recording* adversary cannot
//! recover later, no matter how long they wait. That is the harvest-now-
//! decrypt-later threat, and it is the one that actually applies to a ledger's
//! gossip traffic, because that traffic is worth reading years after capture.
//!
//! Writing a full post-quantum AKE here instead would mean hand-rolling an
//! authenticated protocol, which is the thing not to do casually.
//!
//! ## Transcript binding
//!
//! The derived keys commit to both handshake messages, not just the shared
//! secret. Without that, an attacker who could reflect or splice messages
//! between two concurrent connections might get two sessions to derive the same
//! key from the same encapsulation. Hashing the transcript makes each session's
//! key a function of the exact bytes that session exchanged.
//!
//! ## Implicit rejection, and why nothing branches on it
//!
//! ML-KEM decapsulation never fails. On a forged or corrupted ciphertext FIPS
//! 203 returns a pseudorandom value derived from the ciphertext and a per-key
//! rejection seed, rather than an error. There is deliberately no check here
//! that decapsulation "succeeded", because it always does — a mismatch shows up
//! as the two sides deriving different keys, and the first encrypted frame
//! failing to authenticate.

use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use maya_crypto_pq::kem::{self, CIPHERTEXT_LEN, ENCAPSULATION_KEY_LEN};
use zeroize::Zeroizing;

use crate::error::NodeError;

/// Wire version of the exchange.
///
/// Consensus-adjacent in the sense that both peers must agree, but unlike a
/// transaction format this can be renegotiated: a node that refuses version 1
/// simply fails to connect, and the network can be upgraded peer by peer.
pub const VERSION: u8 = 1;

/// Bytes the responder sends.
pub const RESPONDER_MESSAGE_LEN: usize = 1 + ENCAPSULATION_KEY_LEN;

/// Bytes the initiator sends.
pub const INITIATOR_MESSAGE_LEN: usize = 1 + CIPHERTEXT_LEN;

/// Domain separator for the key derivation.
///
/// BLAKE3's `derive_key` takes a context string that is required to be globally
/// unique and hard-coded — not user input, not a variable. This is that string.
const KDF_CONTEXT: &str = "maya2c 2026-01-01 p2p ml-kem-768 session keys v1";

/// Domain separator prefixed to the transcript, so the transcript hash cannot
/// collide with any other hash the node computes.
const TRANSCRIPT_DOMAIN: &[u8] = b"maya2c.p2p.mlkem768.transcript.v1";

/// Directional session keys.
///
/// Two keys, never one. A single key used in both directions would put the two
/// peers' nonce counters in the same space, and two frames sent with the same
/// (key, nonce) pair destroy ChaCha20-Poly1305's security outright. Deriving
/// separate keys makes that structurally impossible rather than a thing the
/// framing code has to remember.
pub struct SessionKeys {
    /// Key the initiator encrypts with, and the responder decrypts with.
    pub initiator_to_responder: Zeroizing<[u8; 32]>,
    /// Key the responder encrypts with, and the initiator decrypts with.
    pub responder_to_initiator: Zeroizing<[u8; 32]>,
}

impl core::fmt::Debug for SessionKeys {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SessionKeys(<redacted>)")
    }
}

/// Runs the responder half: generate, send, decapsulate.
///
/// The listener side of a connection. Matches libp2p's inbound upgrade.
///
/// # Errors
///
/// Returns [`NodeError::PqHandshake`] if the peer sends an unsupported version,
/// closes early, or the stream fails.
pub async fn respond<S>(stream: &mut S) -> Result<SessionKeys, NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (decapsulation_key, encapsulation_key) = kem::generate_keypair();
    let encoded = encapsulation_key.to_bytes();

    let mut outbound = [0u8; RESPONDER_MESSAGE_LEN];
    outbound[0] = VERSION;
    outbound[1..].copy_from_slice(&encoded);
    write_all(stream, &outbound).await?;

    let mut inbound = [0u8; INITIATOR_MESSAGE_LEN];
    read_exact(stream, &mut inbound).await?;
    check_version(inbound[0])?;

    let mut ciphertext = [0u8; CIPHERTEXT_LEN];
    ciphertext.copy_from_slice(&inbound[1..]);

    // No error branch: see the module docs on implicit rejection. A forged
    // ciphertext yields a different secret here, not a failure, and the
    // mismatch surfaces when the first frame fails to authenticate.
    let secret = decapsulation_key.decapsulate(&ciphertext);

    Ok(derive(&secret, &encoded, &ciphertext))
}

/// Runs the initiator half: receive, encapsulate, send.
///
/// The dialer side of a connection. Matches libp2p's outbound upgrade.
///
/// # Errors
///
/// Returns [`NodeError::PqHandshake`] if the peer sends an unsupported version
/// or a malformed encapsulation key, closes early, or the stream fails.
pub async fn initiate<S>(stream: &mut S) -> Result<SessionKeys, NodeError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut inbound = [0u8; RESPONDER_MESSAGE_LEN];
    read_exact(stream, &mut inbound).await?;
    check_version(inbound[0])?;

    let mut encoded = [0u8; ENCAPSULATION_KEY_LEN];
    encoded.copy_from_slice(&inbound[1..]);

    // This *can* fail, unlike decapsulation: FIPS 203 requires the modulus
    // check on a received encapsulation key, and a peer can send anything.
    let encapsulation_key = kem::EncapsulationKey::from_bytes(&encoded).map_err(|e| {
        NodeError::PqHandshake(format!("peer sent an unusable encapsulation key: {e}"))
    })?;

    let (ciphertext, secret) = encapsulation_key.encapsulate();

    let mut outbound = [0u8; INITIATOR_MESSAGE_LEN];
    outbound[0] = VERSION;
    outbound[1..].copy_from_slice(&ciphertext);
    write_all(stream, &outbound).await?;

    Ok(derive(&secret, &encoded, &ciphertext))
}

/// Derives directional keys from the shared secret and the full transcript.
fn derive(
    secret: &kem::SharedSecret,
    encapsulation_key: &[u8; ENCAPSULATION_KEY_LEN],
    ciphertext: &[u8; CIPHERTEXT_LEN],
) -> SessionKeys {
    let mut hasher = blake3::Hasher::new_derive_key(KDF_CONTEXT);
    hasher.update(secret.as_bytes());
    hasher.update(TRANSCRIPT_DOMAIN);
    hasher.update(&[VERSION]);
    hasher.update(encapsulation_key);
    hasher.update(ciphertext);

    // 64 bytes of output split in half, rather than two hashes with different
    // labels. One XOF read is cheaper and the halves are independent either way.
    let mut okm = Zeroizing::new([0u8; 64]);
    hasher.finalize_xof().fill(okm.as_mut_slice());

    let mut initiator_to_responder = Zeroizing::new([0u8; 32]);
    initiator_to_responder.copy_from_slice(&okm[..32]);

    let mut responder_to_initiator = Zeroizing::new([0u8; 32]);
    responder_to_initiator.copy_from_slice(&okm[32..]);

    SessionKeys {
        initiator_to_responder,
        responder_to_initiator,
    }
}

/// [`derive`], exposed for the dual-KEM combiner's tests.
///
/// The dual path asserts that its own derivation differs from this one — the
/// property that keeps a dual session and an ML-KEM-only session from sharing
/// keys when they share an ML-KEM transcript. Checking that needs both
/// derivations in one place, and `derive` is otherwise private for good reason.
#[cfg(test)]
pub fn derive_for_test(
    secret: &kem::SharedSecret,
    encapsulation_key: &[u8; ENCAPSULATION_KEY_LEN],
    ciphertext: &[u8; CIPHERTEXT_LEN],
) -> SessionKeys {
    derive(secret, encapsulation_key, ciphertext)
}

fn check_version(version: u8) -> Result<(), NodeError> {
    if version == VERSION {
        Ok(())
    } else {
        Err(NodeError::PqHandshake(format!(
            "peer offered ML-KEM handshake version {version}, this node speaks {VERSION}"
        )))
    }
}

async fn read_exact<S>(stream: &mut S, buf: &mut [u8]) -> Result<(), NodeError>
where
    S: AsyncRead + Unpin,
{
    stream
        .read_exact(buf)
        .await
        .map_err(|e| NodeError::PqHandshake(format!("reading handshake: {e}")))
}

async fn write_all<S>(stream: &mut S, buf: &[u8]) -> Result<(), NodeError>
where
    S: AsyncWrite + Unpin,
{
    stream
        .write_all(buf)
        .await
        .map_err(|e| NodeError::PqHandshake(format!("writing handshake: {e}")))?;
    // Flushed explicitly. The peer is blocked reading exactly this many bytes,
    // so a buffered write that never reaches the wire is a deadlock, not a
    // slow start.
    stream
        .flush()
        .await
        .map_err(|e| NodeError::PqHandshake(format!("flushing handshake: {e}")))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use futures::io::Cursor;

    /// A pair of in-memory duplex streams, so the two halves can be driven
    /// against each other without a socket.
    fn duplex() -> (futures_ringbuf::Endpoint, futures_ringbuf::Endpoint) {
        futures_ringbuf::Endpoint::pair(4096, 4096)
    }

    #[tokio::test]
    async fn both_sides_derive_the_same_keys() {
        let (mut a, mut b) = duplex();

        let responder = tokio::spawn(async move { respond(&mut a).await });
        let initiator = initiate(&mut b).await.expect("initiate");
        let responder = responder.await.expect("join").expect("respond");

        assert_eq!(
            *initiator.initiator_to_responder,
            *responder.initiator_to_responder
        );
        assert_eq!(
            *initiator.responder_to_initiator,
            *responder.responder_to_initiator
        );
    }

    #[tokio::test]
    async fn the_two_directions_get_different_keys() {
        // The property that keeps the nonce spaces separate. If these ever
        // matched, the framing layer would be reusing (key, nonce) pairs across
        // the two directions.
        let (mut a, mut b) = duplex();

        let responder = tokio::spawn(async move { respond(&mut a).await });
        let keys = initiate(&mut b).await.expect("initiate");
        responder.await.expect("join").expect("respond");

        assert_ne!(*keys.initiator_to_responder, *keys.responder_to_initiator);
    }

    #[tokio::test]
    async fn two_handshakes_derive_unrelated_keys() {
        // Forward secrecy, as far as a unit test can see it: nothing is carried
        // between connections, so no two sessions share a key.
        let mut first = None;
        let mut second = None;

        for slot in [&mut first, &mut second] {
            let (mut a, mut b) = duplex();
            let responder = tokio::spawn(async move { respond(&mut a).await });
            *slot = Some(initiate(&mut b).await.expect("initiate"));
            responder.await.expect("join").expect("respond");
        }

        assert_ne!(
            *first.unwrap().initiator_to_responder,
            *second.unwrap().initiator_to_responder
        );
    }

    #[tokio::test]
    async fn a_wrong_version_is_refused_by_name() {
        let mut stream = Cursor::new({
            let mut message = vec![0u8; RESPONDER_MESSAGE_LEN];
            message[0] = 0xEE;
            message
        });

        let error = initiate(&mut stream).await.expect_err("must refuse");
        assert!(
            error.to_string().contains("version 238"),
            "unhelpful error: {error}"
        );
    }

    #[tokio::test]
    async fn a_malformed_encapsulation_key_is_refused() {
        // FIPS 203's modulus check, reached through the handshake rather than
        // the KEM wrapper, because this is where a hostile peer meets it.
        let mut stream = Cursor::new({
            let mut message = vec![0xffu8; RESPONDER_MESSAGE_LEN];
            message[0] = VERSION;
            message
        });

        let error = initiate(&mut stream).await.expect_err("must refuse");
        assert!(
            error.to_string().contains("unusable encapsulation key"),
            "unhelpful error: {error}"
        );
    }

    #[tokio::test]
    async fn a_truncated_handshake_is_an_error_not_a_hang() {
        let mut stream = Cursor::new(vec![VERSION, 0x00, 0x00]);
        let error = initiate(&mut stream).await.expect_err("must fail");
        assert!(
            error.to_string().contains("reading handshake"),
            "unhelpful error: {error}"
        );
    }

    #[test]
    fn the_message_widths_are_what_the_framing_assumes() {
        assert_eq!(RESPONDER_MESSAGE_LEN, 1185);
        assert_eq!(INITIATOR_MESSAGE_LEN, 1089);
    }
}
