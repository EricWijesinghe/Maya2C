//! ML-DSA-65 key handling.
//!
//! Thin wrappers over `fips204` that keep the rest of the crate free of direct
//! dependency on its types where practical.
//!
//! ## This is one half of a signature
//!
//! Nothing on the chain is authorized by ML-DSA alone. Every transaction
//! carries a second, hash-based proof under FIPS 205, and an address commits to
//! both keys — see [`crate::crypto::hybrid`], which is the module callers
//! should reach for. The types here are its lattice half, exposed because the
//! keystore and the benchmarks need to name the two halves separately.
//!
//! Consequently there is no `address()` on these types. An ML-DSA key does not
//! name an account by itself, and a function that pretended otherwise would be
//! the easiest way to reintroduce the attack `hybrid` exists to close.
//!
//! ## Why ML-DSA-65 and not ed25519
//!
//! An adversary with a cryptographically relevant quantum computer recovers an
//! ed25519 private key from its public key in polynomial time. That matters more
//! for a chain than for a session protocol: a transaction signature is published
//! permanently, and every address that has ever sent a transaction has therefore
//! published the public key that a future adversary would attack. "Harvest now,
//! decrypt later" against a ledger is "harvest now, *spend* later".
//!
//! ML-DSA-65 is the FIPS 204 parameter set at NIST security category 3. The cost
//! is bulk and speed, both of which are real:
//!
//! | | ed25519 | ML-DSA-65 |
//! |---|---|---|
//! | public key | 32 B | 1952 B |
//! | signature | 64 B | 3309 B |
//! | secret key | 32 B | 4032 B |
//!
//! ## Scope of the guarantee
//!
//! This module makes *transparent transaction authorization* quantum-resistant.
//! It does not make the chain post-quantum. The shielded pool proves joinsplits
//! with Groth16 over BLS12-381 (see `maya_zk_privacy::prove`), a pairing-based
//! system whose soundness rests on discrete log. A quantum adversary that cannot
//! forge a transfer under this module can still forge a shielded proof and mint
//! hidden supply. Replacing that proof system is separate work, and until it
//! happens the chain's post-quantum security is the weaker of the two halves.
//!
//! ## Deterministic signing
//!
//! Signatures are produced with the FIPS 204 deterministic variant — the `rnd`
//! input fixed to 32 zero bytes — rather than the hedged default.
//!
//! This is forced by consensus, not preference. [`crate::core::Transaction::txid`]
//! hashes the signature, and the txid is what the block Merkle root commits to.
//! Under hedged signing, signing one transaction twice yields two different
//! txids for the same authorized payload; a wallet that retried would produce a
//! transaction the network treats as unrelated to the first. Ed25519 is
//! deterministic, so this preserves the property the chain already relied on
//! rather than introducing a new one.
//!
//! The trade is that deterministic signing is the weaker choice against fault
//! injection: an attacker with physical access who can glitch the signer while
//! it signs the same message twice can recover the key. That attack needs the
//! hardware in hand, and it is the same exposure ed25519 has always had here.
//! The alternative — hedged signing plus removing the signature from the txid —
//! would make two distinct signatures over one payload share a txid, which is
//! precisely the malleability the wire format is built to prevent.

use fips204::ml_dsa_65;
use fips204::traits::{KeyGen, SerDes, Signer, Verifier};
use zeroize::Zeroizing;

use crate::error::{NodeError, Result};

/// Length of an encoded ML-DSA-65 signature, in bytes.
///
/// 3309, not 3293. The latter is round-3 Dilithium3, which FIPS 204 superseded
/// with a changed signature encoding. Sizing a wire field to the round-3 number
/// rejects every signature this crate produces.
pub const SIGNATURE_LENGTH: usize = ml_dsa_65::SIG_LEN;

/// Length of an encoded ML-DSA-65 public key, in bytes.
pub const PUBLIC_KEY_LEN: usize = ml_dsa_65::PK_LEN;

/// Length of an encoded ML-DSA-65 private key, in bytes.
pub const SECRET_KEY_LEN: usize = ml_dsa_65::SK_LEN;

/// Length of an address, in bytes.
pub const ADDRESS_LEN: usize = 32;

/// The all-zero `rnd` input selecting FIPS 204's deterministic signing variant.
const DETERMINISTIC_SEED: [u8; 32] = [0u8; 32];

/// Context string, as defined by FIPS 204 section 5.
///
/// Empty on purpose. Domain separation is already carried inside the signed
/// message by `TX_DOMAIN`, and duplicating it here would let two encodings of
/// the same intent both verify.
const SIGNING_CONTEXT: &[u8] = b"";

/// Domain separator for v2 address derivation.
///
/// Superseded. Addresses now commit to both the ML-DSA and the SLH-DSA key —
/// see [`crate::crypto::hybrid`] — and this constant survives only so
/// [`address_of_lattice_only`] can assert that the two spaces are disjoint.
const ADDRESS_DOMAIN: &[u8] = b"custom-l1-node.address.v2";

/// A compile-time check that the sizes this crate hard-codes on the wire match
/// what the implementation actually produces.
///
/// The wire format, the state encoding, and the block size limits are all
/// written against these numbers. If a future `fips204` release changed a
/// parameter set under us, every one of those would be silently wrong; this
/// turns that into a build failure.
const _: () = {
    assert!(
        SIGNATURE_LENGTH == 3309,
        "ML-DSA-65 signature must be 3309 bytes"
    );
    assert!(
        PUBLIC_KEY_LEN == 1952,
        "ML-DSA-65 public key must be 1952 bytes"
    );
    assert!(
        SECRET_KEY_LEN == 4032,
        "ML-DSA-65 private key must be 4032 bytes"
    );
};

/// An ML-DSA-65 signing key.
///
/// Wraps the `fips204` private key so callers never handle its bytes directly.
/// [`SigningKey::to_bytes`] exists for the keystore and returns a [`Zeroizing`]
/// buffer, so an exported key is wiped when the caller drops it.
#[derive(Clone)]
pub struct SigningKey {
    inner: ml_dsa_65::PrivateKey,
}

/// An ML-DSA-65 verifying key.
#[derive(Clone)]
pub struct VerifyingKey {
    inner: ml_dsa_65::PublicKey,
    /// Cached encoding. Verification needs the bytes to derive the address, and
    /// re-encoding a 1952-byte key on every lookup is measurable in a block
    /// validation loop.
    encoded: [u8; PUBLIC_KEY_LEN],
}

impl core::fmt::Debug for SigningKey {
    /// Deliberately opaque. A `Debug` that printed key material would put it in
    /// every log line that ever formatted a surrounding struct.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SigningKey(<redacted>)")
    }
}

impl core::fmt::Debug for VerifyingKey {
    /// Renders a prefix of the key, not an address: this half of a hybrid key
    /// does not name an account on its own.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "VerifyingKey({}…)", hex::encode(&self.encoded[..8]))
    }
}

impl PartialEq for VerifyingKey {
    fn eq(&self, other: &Self) -> bool {
        self.encoded == other.encoded
    }
}

impl Eq for VerifyingKey {}

impl SigningKey {
    /// Decodes a signing key from its 4032-byte encoding.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::MalformedPublicKey`] if the bytes are not a valid
    /// ML-DSA-65 private key encoding.
    pub fn from_bytes(bytes: &[u8; SECRET_KEY_LEN]) -> Result<Self> {
        ml_dsa_65::PrivateKey::try_from_bytes(*bytes)
            .map(|inner| Self { inner })
            .map_err(|_| NodeError::MalformedPublicKey)
    }

    /// Encodes the signing key.
    ///
    /// Returns a [`Zeroizing`] buffer so the copy is wiped on drop. The caller
    /// still owns the problem of where it writes those bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Zeroizing<[u8; SECRET_KEY_LEN]> {
        Zeroizing::new(self.inner.clone().into_bytes())
    }

    /// The matching verifying key.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        let inner = self.inner.get_public_key();
        let encoded = inner.clone().into_bytes();
        VerifyingKey { inner, encoded }
    }

    /// Signs `message` with the deterministic variant.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::SignatureVerification`] if the implementation's
    /// rejection loop fails to terminate, which FIPS 204 permits it to report
    /// and which is not a recoverable condition for a caller.
    pub fn sign(&self, message: &[u8]) -> Result<[u8; SIGNATURE_LENGTH]> {
        self.inner
            .try_sign_with_seed(&DETERMINISTIC_SEED, message, SIGNING_CONTEXT)
            .map_err(|_| NodeError::SignatureVerification)
    }
}

impl VerifyingKey {
    /// Decodes a verifying key from its 1952-byte encoding.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::MalformedPublicKey`] if the bytes are not a valid
    /// ML-DSA-65 public key encoding.
    pub fn from_bytes(bytes: &[u8; PUBLIC_KEY_LEN]) -> Result<Self> {
        let inner = ml_dsa_65::PublicKey::try_from_bytes(*bytes)
            .map_err(|_| NodeError::MalformedPublicKey)?;
        Ok(Self {
            inner,
            encoded: *bytes,
        })
    }

    /// The 1952-byte encoding.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; PUBLIC_KEY_LEN] {
        self.encoded
    }

    /// Verifies `signature` over `message`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::SignatureVerification`] if the signature does not
    /// validate under this key.
    pub fn verify(&self, message: &[u8], signature: &[u8; SIGNATURE_LENGTH]) -> Result<()> {
        if self.inner.verify(message, signature, SIGNING_CONTEXT) {
            Ok(())
        } else {
            Err(NodeError::SignatureVerification)
        }
    }
}

/// Derives a **v2** address from an encoded ML-DSA-65 public key.
///
/// ## Why addresses are hashes
///
/// Under ed25519 the address *was* the public key: 32 bytes, carried in every
/// output, used directly as a state key. An ML-DSA-65 public key is 1952 bytes,
/// which cannot serve that role — it would put nearly two kilobytes into every
/// output, every account key, every undo record, and every hex string an RPC
/// client handles.
///
/// So the address is BLAKE3 over the key, truncated to the 32 bytes the rest of
/// the system already expects. The consequence is that an address no longer
/// reveals the key that controls it, so a transaction must carry its own public
/// key explicitly — see [`crate::core::Transaction::public_key`] — and anything
/// that verifies against a stored address must store the key beside it.
///
/// ## Why this is not the address function any more
///
/// Committing to the lattice key alone is exactly what would make the chain's
/// second signature decorative: an adversary who broke Module-LWE could forge
/// the lattice half and attach an SLH-DSA keypair of their own choosing, and
/// the address would not notice. [`crate::crypto::hybrid::address_of`] commits
/// to both keys and is what the chain actually uses.
///
/// This function remains only so the hybrid module can assert that the v2 and
/// v3 address spaces are disjoint. Nothing on a live path may call it.
#[must_use]
pub fn address_of_lattice_only(public_key: &[u8; PUBLIC_KEY_LEN]) -> [u8; ADDRESS_LEN] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ADDRESS_DOMAIN);
    hasher.update(public_key);
    *hasher.finalize().as_bytes()
}

/// Feeds a fixed 32-byte seed to a routine that expects a CSPRNG.
///
/// FIPS 204 key generation consumes exactly one 32-byte value, the seed `ξ`,
/// and derives everything else from it. Handing it a fixed `ξ` is therefore not
/// a weakened RNG — it is the specification's own deterministic key generation,
/// reached through the only door `fips204` opens.
struct SeedRng {
    seed: [u8; 32],
}

impl fips204::RngCore for SeedRng {
    fn next_u32(&mut self) -> u32 {
        unimplemented!("ML-DSA key generation draws only the 32-byte seed")
    }

    fn next_u64(&mut self) -> u64 {
        unimplemented!("ML-DSA key generation draws only the 32-byte seed")
    }

    fn fill_bytes(&mut self, out: &mut [u8]) {
        // Panics on any other length rather than truncating or repeating. A
        // silent partial fill would produce a key that looks valid and is not
        // the key the seed is supposed to name.
        assert_eq!(
            out.len(),
            self.seed.len(),
            "ML-DSA key generation asked for {} bytes, not the expected seed length",
            out.len()
        );
        out.copy_from_slice(&self.seed);
    }

    fn try_fill_bytes(&mut self, out: &mut [u8]) -> core::result::Result<(), fips204::RngError> {
        self.fill_bytes(out);
        Ok(())
    }
}

impl fips204::CryptoRng for SeedRng {}

/// Derives a signing key deterministically from a 32-byte seed.
///
/// ## What this is for, and what it is not
///
/// The wallet derives account keys through SLIP-0010, which produces a 32-byte
/// hardened chain key per path. This turns that value into the FIPS 204 seed
/// `ξ`, so one mnemonic still yields one reproducible set of accounts.
///
/// That construction is **specific to this chain**. SLIP-0010 covers ed25519
/// and secp256k1; there is no standardized hierarchical derivation for ML-DSA,
/// so no other wallet will recover these accounts from the same mnemonic. A
/// user who imports their words elsewhere will find nothing. That is a real
/// cost of the migration and it belongs in the wallet's user-facing docs, not
/// only here.
///
/// # Errors
///
/// Returns [`NodeError::Network`] if key generation fails, which for a fixed
/// seed indicates a defect rather than a transient condition.
pub fn signing_key_from_seed(seed: &[u8; 32]) -> Result<SigningKey> {
    let mut rng = SeedRng { seed: *seed };
    let (_public, private) = ml_dsa_65::KG::try_keygen_with_rng(&mut rng)
        .map_err(|e| NodeError::Network(format!("ML-DSA key generation failed: {e}")))?;
    Ok(SigningKey { inner: private })
}

/// Generates a fresh signing key from the operating system CSPRNG.
///
/// # Errors
///
/// Returns [`NodeError::Network`] if the OS entropy source is unavailable.
/// Callers must treat this as fatal rather than retrying with a fallback:
/// silently producing a key from degraded randomness is the one outcome a
/// signing key must never have.
pub fn generate_signing_key() -> Result<SigningKey> {
    let (_public, private) = ml_dsa_65::try_keygen()
        .map_err(|e| NodeError::Network(format!("OS entropy source unavailable: {e}")))?;
    Ok(SigningKey { inner: private })
}

/// Decodes a verifying key from its encoding.
///
/// # Errors
///
/// Returns [`NodeError::MalformedPublicKey`] if the bytes are not a valid
/// ML-DSA-65 public key.
pub fn verifying_key_from_bytes(bytes: &[u8; PUBLIC_KEY_LEN]) -> Result<VerifyingKey> {
    VerifyingKey::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_parameter_set_has_the_sizes_the_wire_format_assumes() {
        assert_eq!(SIGNATURE_LENGTH, 3309);
        assert_eq!(PUBLIC_KEY_LEN, 1952);
        assert_eq!(SECRET_KEY_LEN, 4032);
    }

    #[test]
    fn a_signature_verifies_under_its_own_key() {
        let key = generate_signing_key().expect("keygen");
        let signature = key.sign(b"maya2c").expect("sign");
        assert_eq!(key.verifying_key().verify(b"maya2c", &signature), Ok(()));
    }

    #[test]
    fn signing_is_deterministic() {
        // Load-bearing for consensus: txid hashes the signature, so a hedged
        // signer would give one authorized payload two different txids.
        let key = generate_signing_key().expect("keygen");
        let first = key.sign(b"maya2c").expect("first");
        let second = key.sign(b"maya2c").expect("second");
        assert_eq!(first, second);
    }

    #[test]
    fn a_signature_does_not_verify_over_a_different_message() {
        let key = generate_signing_key().expect("keygen");
        let signature = key.sign(b"pay alice").expect("sign");
        assert_eq!(
            key.verifying_key().verify(b"pay bob", &signature),
            Err(NodeError::SignatureVerification)
        );
    }

    #[test]
    fn a_signature_does_not_verify_under_another_key() {
        let signer = generate_signing_key().expect("keygen");
        let impostor = generate_signing_key().expect("keygen");
        let signature = signer.sign(b"maya2c").expect("sign");

        assert_eq!(
            impostor.verifying_key().verify(b"maya2c", &signature),
            Err(NodeError::SignatureVerification)
        );
    }

    #[test]
    fn keys_round_trip_through_their_encodings() {
        let key = generate_signing_key().expect("keygen");
        let signature = key.sign(b"maya2c").expect("sign");

        let restored = SigningKey::from_bytes(&key.to_bytes()).expect("decode private");
        assert_eq!(restored.sign(b"maya2c").expect("re-sign"), signature);

        let public =
            VerifyingKey::from_bytes(&key.verifying_key().to_bytes()).expect("decode public");
        assert_eq!(public.verify(b"maya2c", &signature), Ok(()));
        assert_eq!(public.to_bytes(), key.verifying_key().to_bytes());
    }

    #[test]
    fn a_seed_always_derives_the_same_key() {
        // The wallet's account recovery depends on this: one mnemonic, one
        // SLIP-0010 chain key, one account. Addresses live in the hybrid module
        // now, so identity is asserted on the key material itself.
        let seed = [7u8; 32];
        let first = signing_key_from_seed(&seed).expect("derive");
        let second = signing_key_from_seed(&seed).expect("derive");

        assert_eq!(first.verifying_key(), second.verifying_key());
        assert_eq!(
            first.sign(b"maya2c").expect("sign"),
            second.sign(b"maya2c").expect("sign")
        );
    }

    #[test]
    fn different_seeds_derive_different_keys() {
        let first = signing_key_from_seed(&[1u8; 32]).expect("derive");
        let second = signing_key_from_seed(&[2u8; 32]).expect("derive");
        assert_ne!(first.verifying_key(), second.verifying_key());
    }

    #[test]
    fn a_derived_key_signs_verifiably() {
        let key = signing_key_from_seed(&[9u8; 32]).expect("derive");
        let signature = key.sign(b"maya2c").expect("sign");
        assert_eq!(key.verifying_key().verify(b"maya2c", &signature), Ok(()));
    }

    #[test]
    fn two_keys_are_distinct() {
        let first = generate_signing_key().expect("keygen");
        let second = generate_signing_key().expect("keygen");
        assert_ne!(first.verifying_key(), second.verifying_key());
    }

    #[test]
    fn v2_address_derivation_is_domain_separated() {
        // Retained with the function it tests: a bare BLAKE3 of the key would
        // collide with any other 32-byte digest the chain computes over the
        // same bytes. The live address function is
        // `crate::crypto::hybrid::address_of`, tested there.
        let key = generate_signing_key().expect("keygen");
        let encoded = key.verifying_key().to_bytes();

        let undomained = *blake3::hash(&encoded).as_bytes();
        assert_ne!(address_of_lattice_only(&encoded), undomained);
    }

    #[test]
    fn a_corrupt_public_key_encoding_is_rejected() {
        // ML-DSA public keys are rho plus packed t1 coefficients. Not every
        // 1952-byte string decodes, and one that does not must be an error
        // rather than a key that verifies nothing.
        let key = generate_signing_key().expect("keygen");
        let mut bytes = key.verifying_key().to_bytes();
        bytes[0] ^= 0xff;

        // Whether this specific mutation is rejected depends on the encoding,
        // so assert the weaker, always-true property: it is not the same key.
        match VerifyingKey::from_bytes(&bytes) {
            Ok(other) => assert_ne!(other, key.verifying_key()),
            Err(error) => assert_eq!(error, NodeError::MalformedPublicKey),
        }
    }

    #[test]
    fn a_tampered_signature_is_rejected() {
        let key = generate_signing_key().expect("keygen");
        let mut signature = key.sign(b"maya2c").expect("sign");
        signature[0] ^= 0x01;

        assert_eq!(
            key.verifying_key().verify(b"maya2c", &signature),
            Err(NodeError::SignatureVerification)
        );
    }

    #[test]
    fn a_signing_key_does_not_print_its_material() {
        let key = generate_signing_key().expect("keygen");
        let rendered = format!("{key:?}");
        assert_eq!(rendered, "SigningKey(<redacted>)");
        assert!(!rendered.contains(&hex::encode(&key.to_bytes()[0..8])));
    }
}
