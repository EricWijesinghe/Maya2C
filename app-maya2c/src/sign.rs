//! The lattice half, and the reason there is no whole.
//!
//! # What this can do
//!
//! ML-DSA-65 keygen and signing, from a seed, with no RNG and no heap —
//! `fips204` exposes `keygen_from_seed` and `try_sign_with_seed`, which is
//! exactly the shape a device wants: nothing to seed from the outside, and the
//! same key every time from the same derivation path.
//!
//! # What it cannot do, and why that is the whole finding
//!
//! A Maya2C signature is 11,165 bytes: ML-DSA-65 (3,309) **and**
//! SLH-DSA-SHA2-128s (7,856). `HybridVerifyingKey::verify` checks both. What is
//! here produces the first half, which is not a signature the chain accepts.
//!
//! The second half is behind the `slh-experiment` feature and is expected to
//! fail. `slh-dsa`'s own documentation says it "allocates signatures and
//! intermediate values on the stack, which may cause problems for environments
//! with limited stack space", and its key generation overflowed a **1 MB**
//! stack on a desktop in this project — `src/bin/genesis-ceremony.rs` runs on a
//! 16 MB thread because of it. A Ledger's application RAM is kilobytes.
//!
//! Nothing here pretends otherwise. `sign_hybrid` does not exist; there is no
//! function that returns 11,165 bytes, because there is no way to produce them
//! yet and a stub that returned a padded lattice signature would be a device
//! that looks like it works.
//!
//! # Signing is deterministic, and must stay that way
//!
//! The node signs with a fixed seed (`crypto::keys`), because a transaction id
//! hashes the signature — a hedged signature would give one transaction two
//! identities. `try_sign_with_seed` is what preserves that on the device.

use fips204::ml_dsa_65;
use fips204::traits::{KeyGen, SerDes, Signer};

/// ML-DSA-65 secret key length.
pub const ML_DSA_SK_LEN: usize = 4032;

/// ML-DSA-65 public key length.
pub const ML_DSA_PK_LEN: usize = 1952;

/// ML-DSA-65 signature length.
pub const ML_DSA_SIG_LEN: usize = 3309;

/// The empty context string, matching the node.
const CTX: &[u8] = b"";

/// Derives an ML-DSA-65 key pair from a 32-byte seed.
///
/// Deterministic: the same seed always yields the same key, which is what lets
/// a device reproduce a key from a derivation path rather than storing one.
#[must_use]
pub fn keypair_from_seed(seed: &[u8; 32]) -> ([u8; ML_DSA_PK_LEN], [u8; ML_DSA_SK_LEN]) {
    let (public, secret) = ml_dsa_65::KG::keygen_from_seed(seed);
    (public.into_bytes(), secret.into_bytes())
}

/// Signs `message` with the ML-DSA-65 half.
///
/// **This is not a Maya2C signature.** It is 3,309 of the 11,165 bytes one
/// needs, and a transaction carrying only this is rejected by every node. It
/// exists so the memory cost of the lattice half can be measured.
///
/// # Errors
///
/// `fips204`'s own error string if the secret key does not deserialize or the
/// signature cannot be produced.
pub fn sign_ml_dsa_only(
    secret: &[u8; ML_DSA_SK_LEN],
    message: &[u8],
    seed: &[u8; 32],
) -> Result<[u8; ML_DSA_SIG_LEN], &'static str> {
    let key = ml_dsa_65::PrivateKey::try_from_bytes(*secret)?;
    // Seeded rather than randomised, for the determinism the txid depends on.
    key.try_sign_with_seed(seed, message, CTX)
}
