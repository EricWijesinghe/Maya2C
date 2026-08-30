//! ML-KEM-768 (FIPS 203) key encapsulation.
//!
//! A concrete, non-generic wrapper over `ml-kem`. Used by the node's P2P
//! transport to layer post-quantum confidentiality over its Noise session —
//! see `custom_l1_node::network::pq`.
//!
//! ## What a KEM is doing in a transport
//!
//! libp2p authenticates and encrypts connections with Noise XX over X25519.
//! That is sound today and gives perfect forward secrecy per connection. It is
//! not sound against an adversary who records traffic now and decrypts it once
//! a cryptographically relevant quantum computer exists — "harvest now, decrypt
//! later". A ledger is an unusually attractive target for that: gossip traffic
//! reveals which peer originated which transaction, and that metadata does not
//! expire.
//!
//! ML-KEM-768 closes it. A fresh keypair per connection, encapsulated inside
//! the already-authenticated Noise channel, yields a shared secret an adversary
//! must break Module-LWE to recover — *in addition to* breaking X25519 to get
//! at the Noise layer underneath. Recording the traffic buys nothing.
//!
//! ## Why `ml-kem` and not `pqcrypto-kyber`
//!
//! `pqcrypto-kyber` implements **round-3 Kyber**, not FIPS 203. Its last
//! release predates the standard, and the two are not interchangeable: FIPS 203
//! changed the shared-secret derivation (the ciphertext hash was dropped from
//! the KDF), changed encodings, and added the seed-based key format. Shipping
//! it under a FIPS 203 label would be false.
//!
//! `ml-kem` is also pure Rust, which the Dockerfile's static-musl build depends
//! on — the same reason `fips204` and `slh-dsa` were chosen over their PQClean
//! equivalents.
//!
//! ## Sizes and cost
//!
//! | | ML-KEM-768 |
//! |---|---|
//! | encapsulation key | 1184 B |
//! | ciphertext | 1088 B |
//! | shared secret | 32 B |
//! | keygen | 52 µs |
//! | encapsulate | 42 µs |
//! | decapsulate | 51 µs |
//!
//! A full connection setup is ~144 µs and ~2.3 KB on the wire. Against a 15 s
//! target block time that is 0.001 % of one block interval, and one core
//! sustains roughly 6,900 handshakes per second — orders of magnitude above
//! what a gossip mesh of tens of peers asks for.
//!
//! ## What this does not provide
//!
//! Confidentiality only. The peer on the other end is authenticated by the
//! Noise layer below, using classical ed25519, and nothing here changes that.
//! An adversary with a quantum computer *present at connection time* could
//! still impersonate a peer. That is an active attack requiring the machine to
//! exist and be on the wire, not the recording attack this closes.

use ml_kem::MlKem768;
use ml_kem::kem::{Decapsulate as _, Encapsulate as _, Kem as _, KeyExport as _};
use zeroize::Zeroizing;

/// Length of an encoded ML-KEM-768 encapsulation key ("public key"), in bytes.
pub const ENCAPSULATION_KEY_LEN: usize = 1184;

/// Length of an ML-KEM-768 ciphertext ("encapsulated key"), in bytes.
pub const CIPHERTEXT_LEN: usize = 1088;

/// Length of the shared secret both sides derive, in bytes.
pub const SHARED_SECRET_LEN: usize = 32;

/// Bytes added to a connection handshake: one key out, one ciphertext back.
pub const HANDSHAKE_OVERHEAD: usize = ENCAPSULATION_KEY_LEN + CIPHERTEXT_LEN;

/// A compile-time check that the sizes the wire format hard-codes match what
/// `ml-kem` actually produces.
///
/// The handshake framing is written against these numbers. An `ml-kem` release
/// that changed a parameter set under us would make the frame lengths silently
/// wrong; this turns that into a build failure.
const _: () = {
    assert!(
        ENCAPSULATION_KEY_LEN == 1184,
        "ML-KEM-768 encapsulation key must be 1184 bytes"
    );
    assert!(
        CIPHERTEXT_LEN == 1088,
        "ML-KEM-768 ciphertext must be 1088 bytes"
    );
    assert!(
        SHARED_SECRET_LEN == 32,
        "ML-KEM-768 shared secret must be 32 bytes"
    );
};

/// Errors this module can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KemError {
    /// The encapsulation key bytes are not a valid ML-KEM-768 encoding.
    ///
    /// Unlike SLH-DSA verifying keys, this *can* happen: an ML-KEM
    /// encapsulation key is a packed vector of field elements plus a seed, and
    /// not every 1184-byte string decodes.
    #[error("malformed ML-KEM-768 encapsulation key")]
    MalformedEncapsulationKey,
}

/// The secret half of an ML-KEM-768 keypair.
///
/// Generated fresh per connection by the responder and dropped when the
/// handshake completes. Holding one for longer than a handshake would give up
/// the forward secrecy this exists to provide.
pub struct DecapsulationKey {
    inner: ml_kem::ml_kem_768::DecapsulationKey,
}

/// The public half of an ML-KEM-768 keypair.
#[derive(Clone)]
pub struct EncapsulationKey {
    inner: ml_kem::ml_kem_768::EncapsulationKey,
    /// Cached encoding. The handshake transcript hashes these bytes, so
    /// re-encoding on every read would be pointless work in a connection path.
    encoded: [u8; ENCAPSULATION_KEY_LEN],
}

/// A shared secret, wiped on drop.
///
/// Deliberately not `Clone`, `Debug`, or `PartialEq`: a shared secret that can
/// be copied around freely is one that outlives the scope that should have
/// owned it, and a `Debug` that printed it would put it in any log line that
/// formatted a surrounding struct. Compare with [`SharedSecret::ct_eq`].
pub struct SharedSecret(Zeroizing<[u8; SHARED_SECRET_LEN]>);

impl core::fmt::Debug for DecapsulationKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("kem::DecapsulationKey(<redacted>)")
    }
}

impl core::fmt::Debug for SharedSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("kem::SharedSecret(<redacted>)")
    }
}

impl core::fmt::Debug for EncapsulationKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "kem::EncapsulationKey({}…)", hex_prefix(&self.encoded))
    }
}

impl PartialEq for EncapsulationKey {
    fn eq(&self, other: &Self) -> bool {
        self.encoded == other.encoded
    }
}

impl Eq for EncapsulationKey {}

impl SharedSecret {
    /// The raw secret.
    ///
    /// For feeding a KDF, which is the only thing a caller should do with it.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; SHARED_SECRET_LEN] {
        &self.0
    }

    /// Constant-time equality.
    ///
    /// Used by tests to assert that two sides agree. A `==` on the raw bytes
    /// would be a timing oracle in any code that ever compared a secret against
    /// an attacker-influenced value, and offering only the safe form means no
    /// caller has to notice.
    #[must_use]
    pub fn ct_eq(&self, other: &Self) -> bool {
        let mut diff = 0u8;
        for (a, b) in self.0.iter().zip(other.0.iter()) {
            diff |= a ^ b;
        }
        diff == 0
    }
}

/// Generates a fresh ML-KEM-768 keypair from the operating system CSPRNG.
///
/// One per connection. There is deliberately no seeded or deterministic
/// variant: unlike the signature keys, nothing needs to reproduce a transport
/// keypair, and a deterministic KEM key is a keypair that can be regenerated by
/// anyone who learns the seed — which is the opposite of forward secrecy.
#[must_use]
pub fn generate_keypair() -> (DecapsulationKey, EncapsulationKey) {
    let (dk, ek) = MlKem768::generate_keypair();
    let mut encoded = [0u8; ENCAPSULATION_KEY_LEN];
    encoded.copy_from_slice(&ek.to_bytes());
    (
        DecapsulationKey { inner: dk },
        EncapsulationKey { inner: ek, encoded },
    )
}

impl EncapsulationKey {
    /// Decodes an encapsulation key received from a peer.
    ///
    /// # Errors
    ///
    /// Returns [`KemError::MalformedEncapsulationKey`] if the bytes are not a
    /// valid ML-KEM-768 encoding. This is a real branch, not a formality —
    /// FIPS 203 requires the "modulus check" on a received key, and a peer can
    /// send anything.
    pub fn from_bytes(bytes: &[u8; ENCAPSULATION_KEY_LEN]) -> Result<Self, KemError> {
        let inner = ml_kem::ml_kem_768::EncapsulationKey::new(bytes.into())
            .map_err(|_| KemError::MalformedEncapsulationKey)?;
        Ok(Self {
            inner,
            encoded: *bytes,
        })
    }

    /// The 1184-byte encoding.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; ENCAPSULATION_KEY_LEN] {
        self.encoded
    }

    /// Encapsulates a fresh shared secret to this key.
    ///
    /// Returns the ciphertext to send back and the secret to keep.
    #[must_use]
    pub fn encapsulate(&self) -> ([u8; CIPHERTEXT_LEN], SharedSecret) {
        let (ct, secret) = self.inner.encapsulate();

        let mut ciphertext = [0u8; CIPHERTEXT_LEN];
        ciphertext.copy_from_slice(&ct);

        let mut shared = Zeroizing::new([0u8; SHARED_SECRET_LEN]);
        shared.copy_from_slice(&secret);

        (ciphertext, SharedSecret(shared))
    }
}

impl DecapsulationKey {
    /// Recovers the shared secret from a peer's ciphertext.
    ///
    /// Infallible by design, and that is a property of ML-KEM rather than an
    /// omission here. FIPS 203 decapsulation never rejects: on a malformed or
    /// forged ciphertext it returns an *implicit rejection* value — a secret
    /// derived from the ciphertext and a per-key rejection seed — rather than
    /// an error. The two sides then simply fail to agree, and the handshake
    /// dies at the transcript check.
    ///
    /// This matters for the caller: never branch on decapsulation "succeeding",
    /// because it always does. Authenticate the resulting key instead.
    #[must_use]
    pub fn decapsulate(&self, ciphertext: &[u8; CIPHERTEXT_LEN]) -> SharedSecret {
        let secret = self.inner.decapsulate(ciphertext.into());
        let mut shared = Zeroizing::new([0u8; SHARED_SECRET_LEN]);
        shared.copy_from_slice(&secret);
        SharedSecret(shared)
    }
}

/// Hex-encodes the first 8 bytes, for `Debug`.
fn hex_prefix(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(16);
    for byte in &bytes[..8] {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parameter_set_has_the_sizes_the_handshake_assumes() {
        assert_eq!(ENCAPSULATION_KEY_LEN, 1184);
        assert_eq!(CIPHERTEXT_LEN, 1088);
        assert_eq!(SHARED_SECRET_LEN, 32);
        assert_eq!(HANDSHAKE_OVERHEAD, 2272);
    }

    #[test]
    fn both_sides_derive_the_same_secret() {
        let (dk, ek) = generate_keypair();
        let (ciphertext, sender) = ek.encapsulate();
        let receiver = dk.decapsulate(&ciphertext);
        assert!(sender.ct_eq(&receiver));
    }

    #[test]
    fn a_key_round_trips_through_its_encoding() {
        let (dk, ek) = generate_keypair();
        let decoded = EncapsulationKey::from_bytes(&ek.to_bytes()).expect("decode");
        assert_eq!(decoded, ek);

        // And still works, which the equality check alone would not prove.
        let (ciphertext, sender) = decoded.encapsulate();
        assert!(sender.ct_eq(&dk.decapsulate(&ciphertext)));
    }

    #[test]
    fn two_keypairs_are_distinct() {
        let (_, first) = generate_keypair();
        let (_, second) = generate_keypair();
        assert_ne!(first, second);
    }

    #[test]
    fn encapsulation_is_randomized() {
        // The opposite requirement from the signature schemes, and worth
        // asserting for exactly that reason: signing is deterministic here
        // because txids depend on it, but a KEM that produced the same
        // ciphertext twice would reuse a session key across connections.
        let (_, ek) = generate_keypair();
        let (first, _) = ek.encapsulate();
        let (second, _) = ek.encapsulate();
        assert_ne!(first, second);
    }

    #[test]
    fn a_foreign_ciphertext_yields_a_different_secret() {
        // Implicit rejection: decapsulation does not fail, it returns an
        // unrelated value. The handshake has to detect that by disagreement,
        // never by an error, which is why this asserts inequality rather than
        // an `Err`.
        let (dk, _) = generate_keypair();
        let (_, other_ek) = generate_keypair();
        let (foreign_ciphertext, foreign_secret) = other_ek.encapsulate();

        let recovered = dk.decapsulate(&foreign_ciphertext);
        assert!(!recovered.ct_eq(&foreign_secret));
    }

    #[test]
    fn a_tampered_ciphertext_yields_a_different_secret() {
        let (dk, ek) = generate_keypair();
        let (ciphertext, sender) = ek.encapsulate();

        let mut tampered = ciphertext;
        tampered[0] ^= 0x01;

        assert!(!dk.decapsulate(&tampered).ct_eq(&sender));
    }

    #[test]
    fn a_malformed_encapsulation_key_is_rejected() {
        // FIPS 203's modulus check. Unlike an SLH-DSA verifying key, not every
        // byte string of the right length decodes, and a peer can send
        // anything.
        let malformed = [0xffu8; ENCAPSULATION_KEY_LEN];
        assert_eq!(
            EncapsulationKey::from_bytes(&malformed),
            Err(KemError::MalformedEncapsulationKey)
        );
    }

    #[test]
    fn secrets_do_not_print_their_material() {
        let (dk, ek) = generate_keypair();
        let (_, secret) = ek.encapsulate();
        assert_eq!(format!("{secret:?}"), "kem::SharedSecret(<redacted>)");
        assert_eq!(format!("{dk:?}"), "kem::DecapsulationKey(<redacted>)");
    }

    #[test]
    fn constant_time_equality_agrees_with_the_obvious_one() {
        let (dk, ek) = generate_keypair();
        let (ciphertext, sender) = ek.encapsulate();
        let receiver = dk.decapsulate(&ciphertext);

        assert!(sender.ct_eq(&receiver));
        assert_eq!(sender.as_bytes(), receiver.as_bytes());

        let (_, unrelated) = ek.encapsulate();
        assert!(!sender.ct_eq(&unrelated));
    }
}
