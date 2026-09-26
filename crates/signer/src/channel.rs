//! The node ↔ signer channel: mutually authenticated, post-quantum, over any
//! byte stream (Master Prompt 16 §1).
//!
//! The node's p2p handshake (`network/pq/handshake.rs`) runs ML-KEM inside a
//! Noise session that already authenticated the peer with X25519 — fine for
//! gossip, but a signer's whole job is authentication, so here the identities
//! themselves are post-quantum: each side holds an ML-DSA-65 key the other
//! has **pinned** in its configuration.
//!
//! ```text
//! C → S  m1 = "MSC1" ‖ client_pk ‖ ml_kem_ek ‖ nonce_c
//! S → C  m2 = server_pk ‖ ml_kem_ct ‖ nonce_s ‖ sig_s(h1)      h1 = H(domain ‖ m1 ‖ server_pk ‖ ct ‖ nonce_s)
//! C → S  m3 = sig_c(h2)                                        h2 = H(domain ‖ h1 ‖ sig_s)
//! keys   = derive_key("… c2s" | "… s2c", shared_secret ‖ h2)
//! ```
//!
//! A SIGMA-style signed-KEM exchange: each signature covers the whole
//! transcript so far, so neither side can be spliced into another session.
//! The server refuses an unpinned client **before** any KEM work. Frames are
//! ChaCha20-Poly1305 with a per-direction counter nonce, so a replayed,
//! dropped or reordered frame fails to decrypt.
//!
//! Not externally reviewed. It is the standard shape, written with the
//! standard pieces, and it is tested for the failures it claims to stop; it
//! has not had the cryptographic review a production AKE needs (ADR-022).

use std::io::{Read, Write};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use maya_crypto_pq::kem::{
    CIPHERTEXT_LEN, ENCAPSULATION_KEY_LEN, EncapsulationKey, generate_keypair,
};
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite, SuiteId, verify};
use zeroize::Zeroizing;

const MAGIC: &[u8; 4] = b"MSC1";
const DOMAIN: &str = "maya2c signer handshake v1";
/// Largest handshake or frame accepted, so a hostile peer cannot make us allocate.
const MAX_MSG: usize = 64 * 1024;

/// Channel failures. Every one ends the connection.
#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    /// The stream failed.
    #[error("channel I/O: {0}")]
    Io(#[from] std::io::Error),
    /// The peer is not the key this side pinned.
    #[error("peer identity is not pinned")]
    Unpinned,
    /// A signature, a frame tag or a message shape did not check out.
    #[error("channel authentication failed: {0}")]
    Auth(&'static str),
}

/// This side's long-term ML-DSA-65 identity.
pub struct Identity {
    key: <MlDsa65 as SignatureSuite>::SigningKey,
    public: Vec<u8>,
}

impl Identity {
    /// The identity derived from `seed`.
    #[must_use]
    pub fn from_seed(seed: &MasterSeed) -> Self {
        let key = MlDsa65::signing_key_from_seed(seed);
        let public = MlDsa65::public_key(&key);
        Self { key, public }
    }

    /// The public key the other side pins.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        &self.public
    }

    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, ChannelError> {
        MlDsa65::sign(&self.key, msg).map_err(|_| ChannelError::Auth("signing failed"))
    }
}

fn send_msg<S: Write>(s: &mut S, parts: &[&[u8]]) -> Result<(), ChannelError> {
    let len: usize = parts.iter().map(|p| 4 + p.len()).sum();
    s.write_all(
        &u32::try_from(len)
            .map_err(|_| ChannelError::Auth("oversized"))?
            .to_le_bytes(),
    )?;
    for p in parts {
        s.write_all(
            &u32::try_from(p.len())
                .map_err(|_| ChannelError::Auth("oversized"))?
                .to_le_bytes(),
        )?;
        s.write_all(p)?;
    }
    s.flush()?;
    Ok(())
}

fn recv_msg<S: Read>(s: &mut S, fields: usize) -> Result<Vec<Vec<u8>>, ChannelError> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len)?;
    let len = usize::try_from(u32::from_le_bytes(len)).map_err(|_| ChannelError::Auth("length"))?;
    if len > MAX_MSG {
        return Err(ChannelError::Auth("message too large"));
    }
    let mut body = vec![0u8; len];
    s.read_exact(&mut body)?;
    let mut out = Vec::with_capacity(fields);
    let mut at = 0;
    for _ in 0..fields {
        let n = body
            .get(at..at + 4)
            .ok_or(ChannelError::Auth("truncated"))?;
        let n = usize::try_from(u32::from_le_bytes(
            n.try_into().map_err(|_| ChannelError::Auth("truncated"))?,
        ))
        .map_err(|_| ChannelError::Auth("length"))?;
        out.push(
            body.get(at + 4..at + 4 + n)
                .ok_or(ChannelError::Auth("truncated"))?
                .to_vec(),
        );
        at += 4 + n;
    }
    if at != body.len() {
        return Err(ChannelError::Auth("trailing bytes"));
    }
    Ok(out)
}

fn transcript(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key(DOMAIN);
    for p in parts {
        h.update(&u64::try_from(p.len()).unwrap_or(u64::MAX).to_le_bytes());
        h.update(p);
    }
    *h.finalize().as_bytes()
}

fn nonce32() -> Result<[u8; 32], ChannelError> {
    let mut n = [0u8; 32];
    getrandom::fill(&mut n).map_err(|_| ChannelError::Auth("no entropy"))?;
    Ok(n)
}

/// An established channel.
pub struct Channel<S> {
    stream: S,
    send: ChaCha20Poly1305,
    recv: ChaCha20Poly1305,
    sent: u64,
    received: u64,
    /// The authenticated peer's public key.
    pub peer: Vec<u8>,
}

impl<S: Read + Write> Channel<S> {
    fn new(stream: S, secret: &[u8; 32], h2: &[u8; 32], client: bool, peer: Vec<u8>) -> Self {
        let derive = |label: &str| {
            let mut ikm = Zeroizing::new([0u8; 64]);
            ikm[..32].copy_from_slice(secret);
            ikm[32..].copy_from_slice(h2);
            Zeroizing::new(blake3::derive_key(label, ikm.as_ref()))
        };
        let c2s = derive("maya2c signer channel v1 c2s");
        let s2c = derive("maya2c signer channel v1 s2c");
        let (tx, rx) = if client { (c2s, s2c) } else { (s2c, c2s) };
        Self {
            stream,
            send: ChaCha20Poly1305::new(Key::from_slice(tx.as_ref())),
            recv: ChaCha20Poly1305::new(Key::from_slice(rx.as_ref())),
            sent: 0,
            received: 0,
            peer,
        }
    }

    fn nonce(counter: u64) -> [u8; 12] {
        let mut n = [0u8; 12];
        n[..8].copy_from_slice(&counter.to_le_bytes());
        n
    }

    /// Sends one frame.
    ///
    /// # Errors
    ///
    /// [`ChannelError`] on I/O failure.
    pub fn send(&mut self, plaintext: &[u8]) -> Result<(), ChannelError> {
        let ct = self
            .send
            .encrypt(Nonce::from_slice(&Self::nonce(self.sent)), plaintext)
            .map_err(|_| ChannelError::Auth("encrypt"))?;
        self.sent += 1;
        send_msg(&mut self.stream, &[&ct])
    }

    /// Receives one frame.
    ///
    /// # Errors
    ///
    /// [`ChannelError::Auth`] if the frame was forged, replayed or reordered.
    pub fn recv(&mut self) -> Result<Vec<u8>, ChannelError> {
        let ct = recv_msg(&mut self.stream, 1)?.remove(0);
        let pt = self
            .recv
            .decrypt(
                Nonce::from_slice(&Self::nonce(self.received)),
                ct.as_slice(),
            )
            .map_err(|_| ChannelError::Auth("frame tag"))?;
        self.received += 1;
        Ok(pt)
    }

    /// The underlying stream, for tests that tamper with it.
    pub fn stream_mut(&mut self) -> &mut S {
        &mut self.stream
    }
}

/// The node's side: connect to a signer whose key is `server_pk`.
///
/// # Errors
///
/// [`ChannelError::Unpinned`] if the server presents another key;
/// [`ChannelError::Auth`] for a bad signature or malformed message.
pub fn client<S: Read + Write>(
    mut stream: S,
    me: &Identity,
    server_pk: &[u8],
) -> Result<Channel<S>, ChannelError> {
    let (dk, ek) = generate_keypair();
    let nonce_c = nonce32()?;
    let ek = ek.to_bytes();
    send_msg(&mut stream, &[MAGIC, me.public_key(), &ek, &nonce_c])?;
    let m1 = transcript(&[MAGIC, me.public_key(), &ek, &nonce_c]);
    let f = recv_msg(&mut stream, 4)?;
    let (spk, ct, nonce_s, sig_s) = (&f[0], &f[1], &f[2], &f[3]);
    if spk.as_slice() != server_pk {
        return Err(ChannelError::Unpinned);
    }
    let h1 = transcript(&[&m1, spk, ct, nonce_s]);
    verify(SuiteId::MlDsa65, spk, &h1, sig_s)
        .map_err(|_| ChannelError::Auth("server signature"))?;
    let ct: [u8; CIPHERTEXT_LEN] = ct
        .as_slice()
        .try_into()
        .map_err(|_| ChannelError::Auth("ciphertext length"))?;
    let secret = dk.decapsulate(&ct);
    let h2 = transcript(&[&h1, sig_s]);
    send_msg(&mut stream, &[&me.sign(&h2)?])?;
    Ok(Channel::new(
        stream,
        secret.as_bytes(),
        &h2,
        true,
        spk.clone(),
    ))
}

/// The signer's side: accept a node whose key is one of `allowed`.
///
/// # Errors
///
/// [`ChannelError::Unpinned`] for an unknown client, checked before any KEM
/// work; [`ChannelError::Auth`] for a bad signature or malformed message.
pub fn server<S: Read + Write>(
    mut stream: S,
    me: &Identity,
    allowed: &[Vec<u8>],
) -> Result<Channel<S>, ChannelError> {
    let f = recv_msg(&mut stream, 4)?;
    let (magic, cpk, ek, nonce_c) = (&f[0], &f[1], &f[2], &f[3]);
    if magic.as_slice() != MAGIC {
        return Err(ChannelError::Auth("magic"));
    }
    if !allowed.iter().any(|k| k == cpk) {
        return Err(ChannelError::Unpinned);
    }
    let ek: [u8; ENCAPSULATION_KEY_LEN] = ek
        .as_slice()
        .try_into()
        .map_err(|_| ChannelError::Auth("encapsulation key length"))?;
    let (ct, secret) = EncapsulationKey::from_bytes(&ek)
        .map_err(|_| ChannelError::Auth("encapsulation key"))?
        .encapsulate();
    let nonce_s = nonce32()?;
    let m1 = transcript(&[magic, cpk, &ek, nonce_c]);
    let h1 = transcript(&[&m1, me.public_key(), &ct, &nonce_s]);
    let sig_s = me.sign(&h1)?;
    send_msg(&mut stream, &[me.public_key(), &ct, &nonce_s, &sig_s])?;
    let h2 = transcript(&[&h1, &sig_s]);
    let sig_c = recv_msg(&mut stream, 1)?.remove(0);
    verify(SuiteId::MlDsa65, cpk, &h2, &sig_c)
        .map_err(|_| ChannelError::Auth("client signature"))?;
    Ok(Channel::new(
        stream,
        secret.as_bytes(),
        &h2,
        false,
        cpk.clone(),
    ))
}
