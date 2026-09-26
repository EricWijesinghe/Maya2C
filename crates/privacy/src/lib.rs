//! Selective disclosure with post-quantum viewing keys (Master Prompt 27 §2).
//!
//! A shielded transfer's details (amount, counterparty, memo) are sealed in
//! an [`Envelope`] that only a viewing key opens:
//!
//! - **Per-account viewing key**: an ML-KEM-768 key pair. The account's
//!   viewing (decapsulation) key opens every envelope sealed to its
//!   encapsulation key. Hand it to an accountant and they see the account's
//!   history — and nothing about anyone else's.
//! - **Per-transaction disclosure**: the envelope's own 32-byte key, derived
//!   from the KEM shared secret. Hand it to a tax authority and they can open
//!   **that one** envelope, not the account's others.
//!
//! The chain stores envelopes and commitments; it never holds a viewing key.
//! This is the disclosure layer only: proving that a shielded transfer is
//! valid is the shielded pool's STARK (`maya-zk-stark::pool`), which is dark
//! until its circuit is audited (ADR-016).

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use maya_crypto_pq::kem::{CIPHERTEXT_LEN, DecapsulationKey, EncapsulationKey, generate_keypair};
use zeroize::Zeroizing;

/// Failures.
#[derive(Debug, PartialEq, Eq)]
pub enum PrivacyError {
    /// The key does not open this envelope (wrong key, or tampered).
    CannotOpen,
}

/// An account's viewing key pair.
pub struct ViewingKey {
    dk: DecapsulationKey,
    /// The public half senders seal to.
    pub ek: EncapsulationKey,
}

impl ViewingKey {
    /// A fresh viewing key pair.
    #[must_use]
    pub fn generate() -> Self {
        let (dk, ek) = generate_keypair();
        Self { dk, ek }
    }
}

/// Sealed transfer details.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    kem_ct: [u8; CIPHERTEXT_LEN],
    body: Vec<u8>,
}

impl Envelope {
    /// The stored encoding: KEM ciphertext, then the sealed body.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        [self.kem_ct.as_slice(), &self.body].concat()
    }

    /// Parses a stored envelope; `None` if too short.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let kem_ct = bytes.get(..CIPHERTEXT_LEN)?.try_into().ok()?;
        Some(Self {
            kem_ct,
            body: bytes[CIPHERTEXT_LEN..].to_vec(),
        })
    }
}

/// The per-transaction disclosure key: opens one envelope and nothing else.
#[derive(Clone)]
pub struct DisclosureKey(Zeroizing<[u8; 32]>);

fn envelope_key(secret: &[u8; 32], kem_ct: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut h = blake3::Hasher::new_derive_key("maya2c viewing envelope v1");
    h.update(secret);
    h.update(kem_ct);
    Zeroizing::new(*h.finalize().as_bytes())
}

fn open_with(key: &[u8; 32], env: &Envelope) -> Result<Vec<u8>, PrivacyError> {
    ChaCha20Poly1305::new(Key::from_slice(key))
        .decrypt(Nonce::from_slice(&[0u8; 12]), env.body.as_slice())
        .map_err(|_| PrivacyError::CannotOpen)
}

/// Seals `details` to a viewing key. Each envelope has its own KEM secret,
/// so a fixed zero nonce never repeats under one key.
#[must_use]
pub fn seal(details: &[u8], to: &EncapsulationKey) -> Envelope {
    let (kem_ct, secret) = to.encapsulate();
    let key = envelope_key(secret.as_bytes(), &kem_ct);
    let body = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()))
        .encrypt(Nonce::from_slice(&[0u8; 12]), details)
        .unwrap_or_default();
    Envelope { kem_ct, body }
}

/// Opens with the account viewing key.
///
/// # Errors
///
/// [`PrivacyError::CannotOpen`] for another account's envelope or a tampered one.
pub fn open(env: &Envelope, vk: &ViewingKey) -> Result<Vec<u8>, PrivacyError> {
    let secret = vk.dk.decapsulate(&env.kem_ct);
    open_with(&envelope_key(secret.as_bytes(), &env.kem_ct), env)
}

/// Derives the disclosure key for one envelope (the account holder does
/// this, then shares only the result).
#[must_use]
pub fn disclosure_key(env: &Envelope, vk: &ViewingKey) -> DisclosureKey {
    let secret = vk.dk.decapsulate(&env.kem_ct);
    DisclosureKey(envelope_key(secret.as_bytes(), &env.kem_ct))
}

/// Opens one envelope with its disclosure key.
///
/// # Errors
///
/// [`PrivacyError::CannotOpen`] if the key is for a different envelope.
pub fn open_disclosed(env: &Envelope, key: &DisclosureKey) -> Result<Vec<u8>, PrivacyError> {
    open_with(&key.0, env)
}
