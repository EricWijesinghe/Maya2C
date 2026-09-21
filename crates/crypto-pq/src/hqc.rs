//! HQC-192 (draft FIPS 207) key encapsulation — the second KEM.
//!
//! A concrete, non-generic wrapper over `hqc-kem`, shaped deliberately like
//! [`crate::kem`] so the two can be combined without either side of the
//! handshake special-casing which is which.
//!
//! ## Draft, not standard
//!
//! NIST **selected** HQC in March 2025 as the backup key-encapsulation
//! mechanism to ML-KEM. The specification is **draft FIPS 207**; it is not a
//! published standard, and the parameters and encodings can still move.
//!
//! Every name in this module says draft, and the wire protocol carries a
//! version byte. That is the same discipline `crate::kem` applied when it
//! refused to ship round-3 Kyber under a FIPS 203 label: a handshake whose
//! advertised protocol name misstates its primitive is a trap for whoever
//! audits it next, and "draft standard shipped as standard" is that trap with
//! a longer fuse.
//!
//! ## Why a second KEM at all
//!
//! ML-KEM rests on Module-LWE — a structured lattice assumption. HQC rests on
//! the hardness of decoding random quasi-cyclic codes. The two are unrelated,
//! so an advance against one is not an advance against the other. That is the
//! same reasoning that put ML-DSA and SLH-DSA on every transaction (see
//! `docs/hybrid-signatures.md`), applied to the transport.
//!
//! ## Why HQC-192 and not HQC-128
//!
//! The transport's other KEM is ML-KEM-768, which is NIST level 3. Pairing it
//! with a level 1 code-based KEM would mean the combined channel is only as
//! strong as level 1 against a break in ML-KEM — which is the exact scenario
//! the second KEM exists for. Matching the level is the only choice that makes
//! the pairing mean what it says.
//!
//! The cost is bytes, and it is not small. See [`HANDSHAKE_OVERHEAD`].
//!
//! ## Sizes and cost
//!
//! | | ML-KEM-768 | HQC-192 |
//! |---|---|---|
//! | encapsulation key | 1,184 B | **4,514 B** |
//! | ciphertext | 1,088 B | **8,978 B** |
//! | shared secret | 32 B | 32 B |
//!
//! HQC-192 adds 13,492 bytes to a handshake that was 2,272 — roughly seven
//! times the bytes. `network::pq::measure` reports the segment counts and the
//! wall clock; the headline is that the largest single message is four segments
//! against a ten-segment initial congestion window, so the extra bytes cost
//! bandwidth but **not** a round trip. See `docs/pq-transport.md`.
//!
//! ## What this does not provide
//!
//! Confidentiality only, exactly as [`crate::kem`]. Peer authentication stays
//! classical ed25519 in the Noise layer below, and nothing here changes that.

use hqc_kem::{Hqc192Params, HqcKem};
use rand_core::UnwrapErr;
use zeroize::Zeroizing;

/// The OS CSPRNG, presented to `hqc-kem` as an infallible generator.
///
/// `hqc-kem` takes `&mut impl rand::CryptoRng`, and `CryptoRng` requires
/// `Error = Infallible`. `SysRng` is fallible — reading OS entropy can in
/// principle fail — so it needs an adapter, and `UnwrapErr` is `rand_core`'s
/// own: it panics if the OS refuses entropy.
///
/// Panicking is the correct behaviour here and not a shortcut. A key generated
/// from entropy that failed is not a weaker key, it is a key an attacker may
/// know; continuing with a degraded source would turn a loud failure into a
/// silent compromise. The node cannot make a connection without entropy, and
/// there is nothing to fall back to.
fn os_rng() -> UnwrapErr<rand::rngs::SysRng> {
    UnwrapErr(rand::rngs::SysRng)
}

/// Length of an encoded HQC-192 encapsulation key ("public key"), in bytes.
pub const ENCAPSULATION_KEY_LEN: usize = 4514;

/// Length of an HQC-192 ciphertext ("encapsulated key"), in bytes.
pub const CIPHERTEXT_LEN: usize = 8978;

/// Length of the shared secret both sides derive, in bytes.
///
/// The same 32 as ML-KEM-768's, which is a coincidence of both targeting a
/// 256-bit secret rather than something either specification promises. The
/// compile-time assertion below is what keeps it a fact rather than an
/// assumption.
pub const SHARED_SECRET_LEN: usize = 32;

/// Bytes added to a connection handshake by this KEM alone.
///
/// 13,492 — against ML-KEM-768's 2,272, so a dual handshake is about 15.8 KB
/// where the single one is 2.3 KB.
///
/// That total is **not** the number that decides whether an extra round trip is
/// paid. Congestion control is per-direction, and the largest single message is
/// the responder's 5,699 bytes: four segments at a 1,460-byte MSS, against a
/// ten-segment initial window (RFC 6928). It fits, measured by
/// `network::pq::measure`, and no extra round trip is incurred.
///
/// The bytes still cost bandwidth and still matter on a constrained link. They
/// do not cost a round trip.
pub const HANDSHAKE_OVERHEAD: usize = ENCAPSULATION_KEY_LEN + CIPHERTEXT_LEN;

/// A compile-time check that the sizes the wire format hard-codes match what
/// `hqc-kem` actually produces.
///
/// The handshake framing is written against these numbers, and this crate
/// tracks a **draft** specification — a parameter change between draft
/// revisions is a realistic event, not a hypothetical one. This turns it into a
/// build failure instead of a silently wrong frame length.
const _: () = {
    assert!(
        ENCAPSULATION_KEY_LEN == hqc_kem::hqc192::PUBLIC_KEY_SIZE,
        "HQC-192 encapsulation key size changed under us"
    );
    assert!(
        CIPHERTEXT_LEN == hqc_kem::hqc192::CIPHERTEXT_SIZE,
        "HQC-192 ciphertext size changed under us"
    );
    assert!(
        SHARED_SECRET_LEN == hqc_kem::hqc192::SHARED_SECRET_SIZE,
        "HQC-192 shared secret size changed under us"
    );
};

/// Errors this module can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HqcError {
    /// The encapsulation key bytes are not a valid HQC-192 encoding.
    ///
    /// **Currently unreachable through [`EncapsulationKey::from_bytes`]**, and
    /// that is a property of HQC rather than of this wrapper. Unlike FIPS 203 —
    /// which requires a modulus check that rejects a malformed ML-KEM key —
    /// draft FIPS 207 defines no structural validity condition on an
    /// encapsulation key, so `hqc-kem` checks the length and nothing else. Since
    /// `from_bytes` takes a fixed-size array, the length always matches.
    ///
    /// The variant and the fallible signature stay for two reasons. A later
    /// draft revision may add a check, and this is where it would surface. And
    /// the alternative — an infallible `from_bytes` — would have to be widened
    /// back to a `Result` at every call site if that happened.
    ///
    /// The consequence of no check is not a vulnerability: encapsulating to a
    /// garbage key yields a secret the peer cannot reproduce, the combined
    /// session key differs, and the first frame fails to authenticate. It is
    /// just later and quieter than ML-KEM's rejection.
    #[error("malformed HQC-192 encapsulation key")]
    MalformedEncapsulationKey,
    /// The ciphertext bytes are not a valid HQC-192 encoding.
    #[error("malformed HQC-192 ciphertext")]
    MalformedCiphertext,
}

/// The secret half of an HQC-192 keypair.
///
/// Generated fresh per connection and dropped when the handshake completes,
/// for the same forward-secrecy reason as [`crate::kem::DecapsulationKey`].
pub struct DecapsulationKey {
    inner: hqc_kem::hqc192::DecapsulationKey,
}

/// The public half of an HQC-192 keypair.
#[derive(Clone)]
pub struct EncapsulationKey {
    inner: hqc_kem::hqc192::EncapsulationKey,
    /// Cached encoding. The handshake transcript hashes these bytes, so
    /// re-encoding on every read would be pointless work in a connection path.
    encoded: [u8; ENCAPSULATION_KEY_LEN],
}

/// A shared secret, wiped on drop.
///
/// Deliberately not `Clone`, `Debug`, or `PartialEq`, for the reasons given on
/// [`crate::kem::SharedSecret`]. Compare with [`SharedSecret::ct_eq`].
pub struct SharedSecret(Zeroizing<[u8; SHARED_SECRET_LEN]>);

impl core::fmt::Debug for DecapsulationKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("hqc::DecapsulationKey(<redacted>)")
    }
}

impl core::fmt::Debug for SharedSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("hqc::SharedSecret(<redacted>)")
    }
}

impl core::fmt::Debug for EncapsulationKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "hqc::EncapsulationKey({}…)", hex_prefix(&self.encoded))
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
    /// For feeding a KDF, which is the only thing a caller should do with it —
    /// and in this transport that KDF is the combiner in
    /// `network::pq::handshake`, never this secret alone.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; SHARED_SECRET_LEN] {
        &self.0
    }

    /// Constant-time equality.
    #[must_use]
    pub fn ct_eq(&self, other: &Self) -> bool {
        // `subtle` rather than a hand-written XOR fold: its `black_box`
        // barrier stops the optimiser turning the fold back into an early exit.
        subtle::ConstantTimeEq::ct_eq(self.0.as_slice(), other.0.as_slice()).into()
    }
}

/// Generates a fresh HQC-192 keypair from the operating system CSPRNG.
///
/// One per connection. No seeded variant, for the same reason
/// [`crate::kem::generate_keypair`] has none: a deterministic transport keypair
/// is one anybody who learns the seed can regenerate, which is the opposite of
/// forward secrecy.
#[must_use]
pub fn generate_keypair() -> (DecapsulationKey, EncapsulationKey) {
    let mut rng = os_rng();
    let (ek, dk) = HqcKem::<Hqc192Params>::generate_key(&mut rng);
    let mut encoded = [0u8; ENCAPSULATION_KEY_LEN];
    encoded.copy_from_slice(ek.as_ref());
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
    /// [`HqcError::MalformedEncapsulationKey`] if the bytes are not a valid
    /// HQC-192 encoding.
    pub fn from_bytes(bytes: &[u8; ENCAPSULATION_KEY_LEN]) -> Result<Self, HqcError> {
        let inner = hqc_kem::hqc192::EncapsulationKey::try_from(&bytes[..])
            .map_err(|_| HqcError::MalformedEncapsulationKey)?;
        Ok(Self {
            inner,
            encoded: *bytes,
        })
    }

    /// The canonical encoding, as it goes on the wire and into the transcript.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; ENCAPSULATION_KEY_LEN] {
        self.encoded
    }

    /// Encapsulates to this key, yielding a ciphertext and the shared secret.
    #[must_use]
    pub fn encapsulate(&self) -> ([u8; CIPHERTEXT_LEN], SharedSecret) {
        let mut rng = os_rng();
        let (ct, ss) = self.inner.encapsulate(&mut rng);

        let mut ciphertext = [0u8; CIPHERTEXT_LEN];
        ciphertext.copy_from_slice(ct.as_ref());

        let mut secret = [0u8; SHARED_SECRET_LEN];
        secret.copy_from_slice(ss.as_ref());

        (ciphertext, SharedSecret(Zeroizing::new(secret)))
    }
}

impl DecapsulationKey {
    /// Recovers the shared secret from a peer's ciphertext.
    ///
    /// # Implicit rejection
    ///
    /// Like ML-KEM, HQC's Fujisaki–Okamoto transform means decapsulation
    /// **never fails** — a forged ciphertext yields a pseudorandom value rather
    /// than an error. So nothing here branches on success, and a mismatch
    /// surfaces as the two sides deriving different keys and the first frame
    /// failing to authenticate. That is the same shape as
    /// [`crate::kem::DecapsulationKey::decapsulate`], and it is what keeps the
    /// combined handshake free of a timing oracle on which KEM failed.
    ///
    /// # Errors
    ///
    /// [`HqcError::MalformedCiphertext`] if the bytes do not decode. Note this
    /// is a *decoding* failure, not a decapsulation failure — the distinction
    /// matters because only the first can leak anything, and it leaks only that
    /// the peer sent garbage of the right length.
    pub fn decapsulate(&self, ciphertext: &[u8; CIPHERTEXT_LEN]) -> Result<SharedSecret, HqcError> {
        let ct = hqc_kem::hqc192::Ciphertext::try_from(&ciphertext[..])
            .map_err(|_| HqcError::MalformedCiphertext)?;
        let ss = self.inner.decapsulate(&ct);

        let mut secret = [0u8; SHARED_SECRET_LEN];
        secret.copy_from_slice(ss.as_ref());
        Ok(SharedSecret(Zeroizing::new(secret)))
    }
}

/// First eight bytes as hex, for `Debug`.
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
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_round_trip_agrees_on_the_secret() {
        let (dk, ek) = generate_keypair();
        let (ct, sender_secret) = ek.encapsulate();
        let receiver_secret = dk.decapsulate(&ct).expect("well-formed ciphertext");
        assert!(sender_secret.ct_eq(&receiver_secret));
    }

    #[test]
    fn an_encapsulation_key_round_trips_through_its_encoding() {
        let (_, ek) = generate_keypair();
        let decoded = EncapsulationKey::from_bytes(&ek.to_bytes()).expect("own key decodes");
        assert_eq!(decoded, ek);
    }

    #[test]
    fn two_connections_derive_unrelated_secrets() {
        // Forward secrecy depends on the keypair being fresh per connection.
        // If this ever failed, every session would share one secret.
        let (dk_a, ek_a) = generate_keypair();
        let (dk_b, ek_b) = generate_keypair();
        assert_ne!(ek_a, ek_b);

        let (ct_a, _) = ek_a.encapsulate();
        let (ct_b, _) = ek_b.encapsulate();
        assert_ne!(ct_a.as_slice(), ct_b.as_slice());

        let secret_a = dk_a.decapsulate(&ct_a).expect("well-formed");
        let secret_b = dk_b.decapsulate(&ct_b).expect("well-formed");
        assert!(!secret_a.ct_eq(&secret_b));
    }

    #[test]
    fn a_corrupted_ciphertext_yields_a_different_secret_rather_than_an_error() {
        // Implicit rejection: the FO transform returns a pseudorandom value.
        // A validator branching on "decapsulation failed" would be branching on
        // something that never happens, and would be a timing oracle if it did.
        let (dk, ek) = generate_keypair();
        let (mut ct, sender_secret) = ek.encapsulate();
        ct[0] ^= 0x01;

        let receiver_secret = dk.decapsulate(&ct).expect("still decodes");
        assert!(!sender_secret.ct_eq(&receiver_secret));
    }

    #[test]
    fn the_debug_impls_do_not_print_secrets() {
        let (dk, ek) = generate_keypair();
        let (ct, secret) = ek.encapsulate();

        assert_eq!(format!("{dk:?}"), "hqc::DecapsulationKey(<redacted>)");
        assert_eq!(format!("{secret:?}"), "hqc::SharedSecret(<redacted>)");

        // The encapsulation key is public, so a prefix is fine — but the whole
        // key must not be in the string.
        let rendered = format!("{ek:?}");
        assert!(rendered.starts_with("hqc::EncapsulationKey("));
        assert!(rendered.len() < 64);

        let _ = ct;
    }

    #[test]
    fn any_correctly_sized_string_is_accepted_as_an_encapsulation_key() {
        // Documents a real asymmetry with ML-KEM rather than hiding it. FIPS
        // 203 requires a modulus check and an all-0xff ML-KEM key is refused;
        // draft FIPS 207 defines no such condition, so this succeeds.
        //
        // Harmless, but worth pinning: if a future draft adds a structural
        // check, this test fails and tells whoever bumps the dependency that
        // the handshake's error path just came alive.
        let garbage = [0xffu8; ENCAPSULATION_KEY_LEN];
        let key = EncapsulationKey::from_bytes(&garbage).expect("length is the only check");

        // And it is usable — it just yields a secret nobody else can derive.
        let (_, secret) = key.encapsulate();
        let (dk, _) = generate_keypair();
        let (ct, _) = key.encapsulate();
        let other = dk.decapsulate(&ct).expect("decodes");
        assert!(!secret.ct_eq(&other));
    }

    #[test]
    fn the_handshake_overhead_is_what_the_documentation_claims() {
        // The docs quote this number and the transport plans around it.
        assert_eq!(HANDSHAKE_OVERHEAD, 13_492);
        assert_eq!(ENCAPSULATION_KEY_LEN + CIPHERTEXT_LEN, HANDSHAKE_OVERHEAD);
    }
}
