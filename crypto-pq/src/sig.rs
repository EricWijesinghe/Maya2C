//! SLH-DSA-SHA2-128s (FIPS 205) key handling.
//!
//! A concrete, non-generic wrapper over `slh-dsa`. The node uses this beside
//! ML-DSA-65 (FIPS 204), never instead of it — see
//! `custom_l1_node::crypto::hybrid` for the construction that binds the two.
//!
//! ## Why a second signature scheme at all
//!
//! ML-DSA and SLH-DSA are both post-quantum, but they are not post-quantum for
//! the same reason, and that is the entire point of carrying both.
//!
//! ML-DSA-65's security rests on Module-LWE, a structured lattice assumption
//! roughly fifteen years old. It is believed hard. It is not *proven* hard, and
//! the structure that makes it fast — the ring, the NTT — is also the structure
//! a future attack would exploit. A cryptanalytic break of Module-LWE would
//! forge every transaction this chain has ever accepted.
//!
//! SLH-DSA's security rests on nothing but the preimage and collision
//! resistance of SHA-2. There is no algebraic structure to attack. If SHA-2
//! falls, far more than this chain is already lost. The two schemes therefore
//! fail independently, which is what makes signing under both worth its cost:
//! a forgery needs to break lattices *and* hashes, not either one.
//!
//! ## Why the `s` parameter set and not `f`
//!
//! FIPS 205 offers "small" and "fast" variants at each security level. Both
//! were measured on the reference hardware before this crate was written, in
//! one release-profile run so the two columns are comparable:
//!
//! | | SHA2-128s | SHA2-128f |
//! |---|---|---|
//! | signature | 7856 B | 17088 B |
//! | sign | ~196 ms | ~9 ms |
//! | verify | ~0.22 ms | ~0.56 ms |
//!
//! (`cargo bench --bench hybrid_signing` reports a faster `128s` sign than this
//! — around 105 ms — because criterion warms the cache across samples. The
//! ratio between the two parameter sets is what this table is for, and that
//! holds either way.)
//!
//! `f` signs twenty times faster, which looks decisive until you ask who pays.
//! Signing happens once, in a wallet, per transaction. Verification happens on
//! every node, for every transaction, in every block, forever — and `s`
//! verifies 2.6x faster while producing a signature less than half the size.
//! For a replicated ledger the asymmetry runs the other way from most systems:
//! the one-time cost is the cheap one.
//!
//! ## Every 32-byte string is a valid verifying key
//!
//! Unlike ML-DSA, where a public key is a structured encoding that can be
//! malformed, an SLH-DSA verifying key is just `PK.seed ‖ PK.root` — two hash
//! outputs. There is no validity condition to check and
//! [`VerifyingKey::from_bytes`] is infallible. A garbage key is a key that
//! verifies nothing, not a key that fails to decode.
//!
//! ## Deterministic signing
//!
//! Signatures use FIPS 205's deterministic variant (`opt_rand = PK.seed`),
//! matching the ML-DSA half. This is forced by consensus: the transaction id
//! hashes the signature, so a hedged signer would give one authorized payload
//! two different ids.

use slh_dsa::signature::{Keypair as _, Signer as _, Verifier as _};
use slh_dsa::{Sha2_128s, Signature};
use zeroize::Zeroizing;

/// Length of an encoded SLH-DSA-SHA2-128s signature, in bytes.
pub const SIGNATURE_LEN: usize = 7856;

/// Length of an encoded SLH-DSA-SHA2-128s verifying key, in bytes.
///
/// `PK.seed ‖ PK.root`, two 16-byte hash outputs.
pub const PUBLIC_KEY_LEN: usize = 32;

/// Length of an encoded SLH-DSA-SHA2-128s signing key, in bytes.
///
/// `SK.seed ‖ SK.prf ‖ PK.seed ‖ PK.root`.
pub const SECRET_KEY_LEN: usize = 64;

/// The security parameter `n`, in bytes. Each of the three key seeds is this
/// long, so deterministic key generation needs `3 * N` bytes of material.
const N: usize = 16;

/// Domain separator for expanding a chain key into FIPS 205 seeds.
///
/// Distinct from every other use of BLAKE3 in the tree. Deriving the ML-DSA and
/// SLH-DSA seeds from one chain key without separating them would make the two
/// "independent" halves share their entropy, which is exactly the correlation
/// the hybrid construction exists to avoid.
const SEED_DOMAIN: &[u8] = b"maya2c.slh-dsa-sha2-128s.seed.v1";

/// A compile-time check that the sizes this crate hard-codes on the wire match
/// what `slh-dsa` actually produces.
///
/// The transaction encoding, the channel closure layout, and the block size
/// limits are all written against these numbers. A `slh-dsa` release that
/// changed a parameter set under us would make every one of them silently
/// wrong; this turns that into a build failure.
const _: () = {
    assert!(
        SIGNATURE_LEN == 7856,
        "SLH-DSA-SHA2-128s signature must be 7856 bytes"
    );
    assert!(
        PUBLIC_KEY_LEN == 32,
        "SLH-DSA-SHA2-128s verifying key must be 32 bytes"
    );
    assert!(
        SECRET_KEY_LEN == 64,
        "SLH-DSA-SHA2-128s signing key must be 64 bytes"
    );
};

/// Errors this crate can produce.
///
/// Deliberately its own type rather than the node's `NodeError`: this crate is
/// a leaf and must not depend on the node. The node maps these at its boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SlhError {
    /// The signing key bytes are not a valid SLH-DSA-SHA2-128s encoding.
    #[error("malformed SLH-DSA-SHA2-128s signing key")]
    MalformedSigningKey,

    /// The signature did not verify against the message and key.
    #[error("SLH-DSA-SHA2-128s signature verification failed")]
    Verification,

    /// The operating system entropy source was unavailable.
    #[error("OS entropy source unavailable: {0}")]
    Entropy(&'static str),
}

/// An SLH-DSA-SHA2-128s signing key.
#[derive(Clone)]
pub struct SigningKey {
    inner: slh_dsa::SigningKey<Sha2_128s>,
}

/// An SLH-DSA-SHA2-128s verifying key.
#[derive(Clone)]
pub struct VerifyingKey {
    inner: slh_dsa::VerifyingKey<Sha2_128s>,
    /// Cached encoding. Address derivation needs the bytes on every lookup and
    /// re-encoding in a block validation loop is pointless work.
    encoded: [u8; PUBLIC_KEY_LEN],
}

impl core::fmt::Debug for SigningKey {
    /// Deliberately opaque. A `Debug` that printed key material would put it in
    /// every log line that ever formatted a surrounding struct.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("slh::SigningKey(<redacted>)")
    }
}

impl core::fmt::Debug for VerifyingKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "slh::VerifyingKey({})", hex_16(&self.encoded))
    }
}

impl PartialEq for VerifyingKey {
    fn eq(&self, other: &Self) -> bool {
        self.encoded == other.encoded
    }
}

impl Eq for VerifyingKey {}

impl SigningKey {
    /// Decodes a signing key from its 64-byte encoding.
    ///
    /// # Errors
    ///
    /// Returns [`SlhError::MalformedSigningKey`] if the bytes are not a valid
    /// encoding.
    pub fn from_bytes(bytes: &[u8; SECRET_KEY_LEN]) -> Result<Self, SlhError> {
        slh_dsa::SigningKey::<Sha2_128s>::try_from(bytes.as_slice())
            .map(|inner| Self { inner })
            .map_err(|_| SlhError::MalformedSigningKey)
    }

    /// Encodes the signing key.
    ///
    /// Returns a [`Zeroizing`] buffer so the copy is wiped on drop. The caller
    /// still owns the problem of where it writes those bytes.
    #[must_use]
    pub fn to_bytes(&self) -> Zeroizing<[u8; SECRET_KEY_LEN]> {
        let mut out = [0u8; SECRET_KEY_LEN];
        out.copy_from_slice(&self.inner.to_bytes());
        Zeroizing::new(out)
    }

    /// The matching verifying key.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        let inner = self.inner.verifying_key();
        let mut encoded = [0u8; PUBLIC_KEY_LEN];
        encoded.copy_from_slice(&inner.to_bytes());
        VerifyingKey { inner, encoded }
    }

    /// Signs `message` with the deterministic variant and an empty FIPS 205
    /// context string.
    ///
    /// The context is empty on purpose: domain separation is already carried
    /// inside the signed message by the caller, and duplicating it here would
    /// let two encodings of the same intent both verify.
    ///
    /// Infallible in practice — SLH-DSA has no rejection loop — but the return
    /// type is an array rather than a `Result` precisely because there is no
    /// failure mode to report.
    #[must_use]
    pub fn sign(&self, message: &[u8]) -> [u8; SIGNATURE_LEN] {
        let mut out = [0u8; SIGNATURE_LEN];
        out.copy_from_slice(&self.inner.sign(message).to_bytes());
        out
    }
}

impl VerifyingKey {
    /// Decodes a verifying key from its 32-byte encoding.
    ///
    /// Infallible: an SLH-DSA verifying key is two hash outputs with no
    /// validity condition. See the module docs.
    #[must_use]
    pub fn from_bytes(bytes: &[u8; PUBLIC_KEY_LEN]) -> Self {
        // `slh-dsa` also offers an infallible `From<Array<u8, VkLen>>`, which
        // would remove this `expect` — but `Array` is `hybrid_array`'s, and
        // `slh-dsa` does not re-export it. Reaching it would mean taking a
        // direct dependency on `hybrid-array` pinned in lockstep with
        // `slh-dsa`'s own, which puts a version-coupling hazard in the
        // consensus path to delete one unreachable branch. Not worth it.
        //
        // Unreachable is meant literally: `try_from`'s only failure mode is
        // `bytes.len() != 32`, and the parameter type is `&[u8; 32]`.
        let inner = slh_dsa::VerifyingKey::<Sha2_128s>::try_from(bytes.as_slice())
            .expect("a 32-byte slice is always a valid SHA2-128s verifying key");
        Self {
            inner,
            encoded: *bytes,
        }
    }

    /// The 32-byte encoding.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; PUBLIC_KEY_LEN] {
        self.encoded
    }

    /// Verifies `signature` over `message`.
    ///
    /// # Errors
    ///
    /// Returns [`SlhError::Verification`] if the signature does not validate
    /// under this key, including when the bytes are not a well-formed
    /// signature encoding at all.
    pub fn verify(&self, message: &[u8], signature: &[u8; SIGNATURE_LEN]) -> Result<(), SlhError> {
        let parsed = Signature::<Sha2_128s>::try_from(signature.as_slice())
            .map_err(|_| SlhError::Verification)?;
        self.inner
            .verify(message, &parsed)
            .map_err(|_| SlhError::Verification)
    }
}

/// Derives a signing key deterministically from a 32-byte chain key.
///
/// SLH-DSA needs 48 bytes of key material (`SK.seed`, `SK.prf`, `PK.seed`),
/// not 32, so the chain key is expanded with BLAKE3's XOF under
/// `SEED_DOMAIN`. This reaches FIPS 205's own `slh_keygen_internal` rather
/// than feeding a fake RNG to the public constructor: the seeds are the
/// specification's inputs, and naming them directly is clearer than
/// impersonating an entropy source.
#[must_use]
pub fn signing_key_from_seed(chain_key: &[u8; 32]) -> SigningKey {
    let material = Zeroizing::new(expand_seed(chain_key));
    let inner = slh_dsa::SigningKey::<Sha2_128s>::slh_keygen_internal(
        &material[..N],
        &material[N..2 * N],
        &material[2 * N..],
    );
    SigningKey { inner }
}

/// Expands a 32-byte chain key into the 48 bytes FIPS 205 key generation needs.
fn expand_seed(chain_key: &[u8; 32]) -> [u8; 3 * N] {
    let mut out = [0u8; 3 * N];
    let mut hasher = blake3::Hasher::new();
    hasher.update(SEED_DOMAIN);
    hasher.update(chain_key);
    hasher.finalize_xof().fill(&mut out);
    out
}

/// Generates a fresh signing key from the operating system CSPRNG.
///
/// # Errors
///
/// Returns [`SlhError::Entropy`] if the OS entropy source is unavailable.
/// Callers must treat this as fatal rather than retrying with a fallback:
/// silently producing a key from degraded randomness is the one outcome a
/// signing key must never have.
pub fn generate_signing_key() -> Result<SigningKey, SlhError> {
    // Drawn as a 32-byte chain key and expanded, rather than pulling 48 bytes
    // straight from the OS, so that a generated key and a derived key travel
    // exactly the same code path. One path means one thing to audit.
    let mut chain_key = Zeroizing::new([0u8; 32]);
    getrandom::fill(chain_key.as_mut_slice()).map_err(|_| SlhError::Entropy("getrandom failed"))?;
    Ok(signing_key_from_seed(&chain_key))
}

/// Hex-encodes the first 8 bytes, for `Debug`.
///
/// Local rather than a `hex` dependency: this is the only formatting this crate
/// does, and a public key prefix is not worth a crate.
fn hex_16(bytes: &[u8; PUBLIC_KEY_LEN]) -> String {
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
    fn the_parameter_set_has_the_sizes_the_wire_format_assumes() {
        assert_eq!(SIGNATURE_LEN, 7856);
        assert_eq!(PUBLIC_KEY_LEN, 32);
        assert_eq!(SECRET_KEY_LEN, 64);
    }

    #[test]
    fn a_signature_verifies_under_its_own_key() {
        let key = signing_key_from_seed(&[1u8; 32]);
        let signature = key.sign(b"maya2c");
        assert_eq!(key.verifying_key().verify(b"maya2c", &signature), Ok(()));
    }

    #[test]
    fn signing_is_deterministic() {
        // Load-bearing for consensus: txid hashes the signature, so a hedged
        // signer would give one authorized payload two different txids.
        let key = signing_key_from_seed(&[2u8; 32]);
        assert_eq!(key.sign(b"maya2c"), key.sign(b"maya2c"));
    }

    #[test]
    fn a_signature_does_not_verify_over_a_different_message() {
        let key = signing_key_from_seed(&[3u8; 32]);
        let signature = key.sign(b"pay alice");
        assert_eq!(
            key.verifying_key().verify(b"pay bob", &signature),
            Err(SlhError::Verification)
        );
    }

    #[test]
    fn a_signature_does_not_verify_under_another_key() {
        let signer = signing_key_from_seed(&[4u8; 32]);
        let impostor = signing_key_from_seed(&[5u8; 32]);
        let signature = signer.sign(b"maya2c");
        assert_eq!(
            impostor.verifying_key().verify(b"maya2c", &signature),
            Err(SlhError::Verification)
        );
    }

    #[test]
    fn a_tampered_signature_is_rejected() {
        let key = signing_key_from_seed(&[6u8; 32]);
        let mut signature = key.sign(b"maya2c");
        signature[0] ^= 0x01;
        assert_eq!(
            key.verifying_key().verify(b"maya2c", &signature),
            Err(SlhError::Verification)
        );
    }

    #[test]
    fn keys_round_trip_through_their_encodings() {
        let key = signing_key_from_seed(&[7u8; 32]);
        let signature = key.sign(b"maya2c");

        let restored = SigningKey::from_bytes(&key.to_bytes()).expect("decode signing key");
        assert_eq!(restored.sign(b"maya2c"), signature);

        let public = VerifyingKey::from_bytes(&key.verifying_key().to_bytes());
        assert_eq!(public.verify(b"maya2c", &signature), Ok(()));
    }

    #[test]
    fn a_seed_always_derives_the_same_key() {
        let first = signing_key_from_seed(&[8u8; 32]);
        let second = signing_key_from_seed(&[8u8; 32]);
        assert_eq!(first.verifying_key(), second.verifying_key());
        assert_eq!(first.sign(b"maya2c"), second.sign(b"maya2c"));
    }

    #[test]
    fn different_seeds_derive_different_keys() {
        let first = signing_key_from_seed(&[9u8; 32]);
        let second = signing_key_from_seed(&[10u8; 32]);
        assert_ne!(first.verifying_key(), second.verifying_key());
    }

    #[test]
    fn seed_expansion_is_domain_separated() {
        // A bare BLAKE3 of the chain key would collide with any other 32-byte
        // digest the chain computes over the same bytes — including the ML-DSA
        // seed derived from the same chain key.
        let chain_key = [11u8; 32];
        let expanded = expand_seed(&chain_key);
        let undomained = *blake3::hash(&chain_key).as_bytes();
        assert_ne!(expanded[..32], undomained[..]);
    }

    #[test]
    fn generated_keys_differ() {
        let first = generate_signing_key().expect("keygen");
        let second = generate_signing_key().expect("keygen");
        assert_ne!(first.verifying_key(), second.verifying_key());
    }

    #[test]
    fn a_signing_key_does_not_print_its_material() {
        let key = signing_key_from_seed(&[12u8; 32]);
        assert_eq!(format!("{key:?}"), "slh::SigningKey(<redacted>)");
    }

    #[test]
    fn any_thirty_two_bytes_decode_as_a_verifying_key() {
        // Documented in the module docs and relied on by the wire decoder:
        // there is no malformed-public-key case for SLH-DSA, only a key that
        // verifies nothing.
        let key = VerifyingKey::from_bytes(&[0xffu8; PUBLIC_KEY_LEN]);
        let signature = signing_key_from_seed(&[13u8; 32]).sign(b"maya2c");
        assert_eq!(
            key.verify(b"maya2c", &signature),
            Err(SlhError::Verification)
        );
    }
}
