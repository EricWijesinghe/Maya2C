//! Sealing a share to exactly one custodian.
//!
//! # Why the transport is not the protection
//!
//! The brief for this module said "over TLS". TLS protects a *hop*. A share
//! travels from its dealer to its recipient through whatever coordinates the
//! ceremony — a relay, a queue, an operator's laptop — and every one of those
//! terminates the TLS session and sees plaintext. A custody protocol whose
//! confidentiality argument is "the connection was encrypted" has a trust
//! boundary drawn around the wrong thing.
//!
//! So each share is sealed to its recipient's own key before it is handed to
//! any transport, and the transport carries an already-opaque blob. TLS then
//! adds what it is actually good for — authenticating the peer and hiding the
//! traffic pattern — rather than being asked to keep the secret. See
//! [`crate::tls`].
//!
//! # Why ML-KEM and not X25519
//!
//! A share sealed under X25519 in 2026 is a share readable by whoever recorded
//! it, once a quantum computer exists. On this chain that is not a hypothetical
//! objection — it is the reason the chain exists. ML-KEM-768 is what the node's
//! own p2p transport already uses (`src/network/pq/handshake.rs`), through the
//! same `maya-crypto-pq` wrapper, so this introduces no new primitive.
//!
//! The Pedersen commitments in [`crate::vss`] are perfectly hiding for the same
//! reason: nothing published or transmitted during a ceremony may become
//! readable later.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use maya_crypto_pq::kem::{self, CIPHERTEXT_LEN, ENCAPSULATION_KEY_LEN};
use zeroize::Zeroizing;

use crate::error::{CustodyError, Result};
use crate::vss::{SHARE_BODY_LEN, ShareBody};

/// Context string for the KEM-output-to-AEAD-key derivation.
const KEY_DOMAIN: &[u8] = b"maya2c.custody-mpc.share-seal.ml-kem-768.chacha20poly1305.v1";

/// Context string for the additional authenticated data.
const AAD_DOMAIN: &[u8] = b"maya2c.custody-mpc.share-seal.aad.v1";

/// Encoded length of a sealed share.
pub const SEALED_SHARE_LEN: usize = CIPHERTEXT_LEN + SHARE_BODY_LEN + 16;

/// A share encrypted to one custodian.
///
/// Opaque to everyone else, including whatever relays it. The dealer and
/// recipient indices travel in the clear because they are routing information,
/// and they are also bound into the AEAD's additional data — so a relay may
/// read them and may not rewrite them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedShare {
    /// Who dealt it.
    pub dealer: u8,
    /// Who may open it.
    pub recipient: u8,
    /// ML-KEM ciphertext followed by the AEAD ciphertext and tag.
    pub body: Vec<u8>,
}

/// A custodian's ephemeral KEM key pair for one ceremony.
///
/// Ephemeral on purpose. `maya_crypto_pq::kem::DecapsulationKey` has no
/// serialisation, which means a ceremony key cannot be written to disk, cannot
/// outlive the process, and cannot be reused by a later ceremony by accident.
/// That is a smaller and more honest key-management surface than a long-lived
/// custodian KEM identity would be, and the authenticity that a long-lived key
/// would have provided comes from the mutually-authenticated transport instead.
pub struct CeremonyKey {
    decapsulation: kem::DecapsulationKey,
    encapsulation: kem::EncapsulationKey,
}

impl core::fmt::Debug for CeremonyKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CeremonyKey")
            .field("encapsulation", &"<public>")
            .field("decapsulation", &"<redacted>")
            .finish()
    }
}

impl CeremonyKey {
    /// Draws a fresh ceremony key pair.
    #[must_use]
    pub fn generate() -> Self {
        let (decapsulation, encapsulation) = kem::generate_keypair();
        Self {
            decapsulation,
            encapsulation,
        }
    }

    /// The public half, to publish in round one.
    #[must_use]
    pub fn encapsulation_key(&self) -> [u8; ENCAPSULATION_KEY_LEN] {
        self.encapsulation.to_bytes()
    }
}

/// Additional authenticated data binding a sealed share to its place.
///
/// Ceremony, dealer, and recipient. Without this a sealed share is a blob that
/// verifies wherever it is replayed: an attacker who could re-route custodian
/// 2's share to slot 4 would produce a vault whose shares disagree, and the
/// failure would surface as an unexplained bad reconstruction months later.
fn aad(ceremony: &[u8; 32], dealer: u8, recipient: u8) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(AAD_DOMAIN);
    hasher.update(ceremony);
    hasher.update(&[dealer, recipient]);
    *hasher.finalize().as_bytes()
}

/// Derives the AEAD key from the KEM shared secret.
///
/// The nonce is all zeros and that is correct here, not a shortcut: the key is
/// derived from a shared secret produced by one encapsulation, is used for
/// exactly one message, and is then dropped. Nonce reuse requires key reuse,
/// and there is no path on which this key is used twice. A random nonce would
/// add 12 bytes to the wire and nothing to the security argument.
fn aead_key(shared: &kem::SharedSecret, ceremony: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(KEY_DOMAIN);
    hasher.update(ceremony);
    hasher.update(shared.as_bytes());
    Zeroizing::new(*hasher.finalize().as_bytes())
}

/// Seals `share` to the holder of `encapsulation_key`.
///
/// # Errors
///
/// [`CustodyError::SealFailed`] if the recipient's encapsulation key is not a
/// valid encoding, or if the AEAD refuses — neither of which a correct caller
/// can produce, and both of which are the recipient's problem to explain.
pub fn seal(
    share: &ShareBody,
    encapsulation_key: &[u8; ENCAPSULATION_KEY_LEN],
    ceremony: &[u8; 32],
    dealer: u8,
) -> Result<SealedShare> {
    let recipient = share.index;
    let key = kem::EncapsulationKey::from_bytes(encapsulation_key)
        .map_err(|_| CustodyError::SealFailed(recipient))?;
    let (kem_ciphertext, shared) = key.encapsulate();

    let cipher = ChaCha20Poly1305::new(Key::from_slice(aead_key(&shared, ceremony).as_slice()));
    let plaintext = share.to_bytes();
    let sealed = cipher
        .encrypt(
            Nonce::from_slice(&[0u8; 12]),
            Payload {
                msg: plaintext.as_slice(),
                aad: &aad(ceremony, dealer, recipient),
            },
        )
        .map_err(|_| CustodyError::SealFailed(recipient))?;

    let mut body = Vec::with_capacity(SEALED_SHARE_LEN);
    body.extend_from_slice(&kem_ciphertext);
    body.extend_from_slice(&sealed);

    Ok(SealedShare {
        dealer,
        recipient,
        body,
    })
}

/// Opens a sealed share.
///
/// # Errors
///
/// [`CustodyError::SealFailed`], attributed to the *dealer*, if the ciphertext
/// is the wrong length, was not encapsulated to this key, or fails the AEAD
/// tag. The three are deliberately one error: which of them it was is
/// information an attacker would like and an operator cannot act on
/// differently.
///
/// [`CustodyError::Malformed`] if the plaintext is not a well-formed share —
/// which, past a passing AEAD tag, means the dealer encoded it wrongly.
pub fn open(sealed: &SealedShare, key: &CeremonyKey, ceremony: &[u8; 32]) -> Result<ShareBody> {
    let dealer = sealed.dealer;
    if sealed.body.len() != SEALED_SHARE_LEN {
        return Err(CustodyError::SealFailed(dealer));
    }

    let mut kem_ciphertext = [0u8; CIPHERTEXT_LEN];
    kem_ciphertext.copy_from_slice(&sealed.body[..CIPHERTEXT_LEN]);
    // ML-KEM decapsulation is implicitly rejecting: a ciphertext that was not
    // produced for this key yields a shared secret rather than an error, and it
    // is the AEAD tag below that notices. That is the design, not a gap.
    let shared = key.decapsulation.decapsulate(&kem_ciphertext);

    let cipher = ChaCha20Poly1305::new(Key::from_slice(aead_key(&shared, ceremony).as_slice()));
    let plaintext = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&[0u8; 12]),
                Payload {
                    msg: &sealed.body[CIPHERTEXT_LEN..],
                    aad: &aad(ceremony, dealer, sealed.recipient),
                },
            )
            .map_err(|_| CustodyError::SealFailed(dealer))?,
    );

    let mut body = Zeroizing::new([0u8; SHARE_BODY_LEN]);
    if plaintext.len() != SHARE_BODY_LEN {
        return Err(CustodyError::Malformed("sealed share of the wrong length"));
    }
    body.copy_from_slice(&plaintext);
    ShareBody::from_bytes(&body)
}
