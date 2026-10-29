//! ML-DSA-87 validator keys (ADR-051 option 3, approved 2026-10-28).
//!
//! Validator and BFT vote keys are long-lived and few, so they take FIPS 204's
//! category-5 parameter set, the highest NIST defines. Transaction keys stay in
//! [`crate::crypto::keys`] (ML-DSA-65, one half of the hybrid with SLH-DSA): every
//! transaction carries its signature, while a vote certificate is a small,
//! bounded set of them, so the larger size costs consensus far less.
//!
//! The backend is `maya_crypto_pq::suite::validator` (invariant 4: ML-DSA-87
//! never comes through `fips204`). Signatures are bound to the FIPS 204 context
//! `maya2c validator v1`, so a validator signature never verifies as any other
//! signature over the same bytes.
//!
//! The API mirrors [`crate::crypto::keys`]. They are distinct types on purpose:
//! a transaction key cannot be passed where a validator key is expected.
//!
//! | | ML-DSA-65 (transactions) | ML-DSA-87 (validators) |
//! |---|---|---|
//! | NIST category | 3 | 5 |
//! | public key | 1,952 B | 2,592 B |
//! | signature | 3,309 B | 4,627 B |
//! | secret at rest | 4,032 B expanded | 32 B seed |

use maya_crypto_pq::suite::validator::{self as backend, ValidatorSigningKey};
use zeroize::Zeroizing;

use crate::error::{NodeError, Result};

/// Encoded signature length.
pub const SIGNATURE_LENGTH: usize = backend::SIGNATURE_LEN;

/// Encoded public key length.
pub const PUBLIC_KEY_LEN: usize = backend::PUBLIC_KEY_LEN;

/// Secret key length at rest: the FIPS 204 seed `ξ`.
pub const SECRET_KEY_LEN: usize = backend::SEED_LEN;

const _: () = {
    assert!(
        SIGNATURE_LENGTH == 4627,
        "ML-DSA-87 signature must be 4627 bytes"
    );
    assert!(
        PUBLIC_KEY_LEN == 2592,
        "ML-DSA-87 public key must be 2592 bytes"
    );
};

/// A validator signing key.
#[derive(Clone)]
pub struct SigningKey {
    inner: ValidatorSigningKey,
}

/// A validator verifying key.
#[derive(Clone, PartialEq, Eq)]
pub struct VerifyingKey {
    encoded: [u8; PUBLIC_KEY_LEN],
}

impl core::fmt::Debug for SigningKey {
    /// Opaque: a `Debug` that printed key material would leak it into logs.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SigningKey(<redacted>)")
    }
}

impl core::fmt::Debug for VerifyingKey {
    /// A prefix: enough to tell validators apart in a log.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "VerifyingKey({}…)", hex::encode(&self.encoded[..8]))
    }
}

impl SigningKey {
    /// The key a 32-byte seed names.
    ///
    /// # Errors
    ///
    /// Never today; `Result` keeps the signature of [`crate::crypto::keys`].
    pub fn from_bytes(bytes: &[u8; SECRET_KEY_LEN]) -> Result<Self> {
        Ok(Self {
            inner: ValidatorSigningKey::from_seed(bytes),
        })
    }

    /// The seed, for the sealed keystore; wiped when dropped.
    #[must_use]
    pub fn to_bytes(&self) -> Zeroizing<[u8; SECRET_KEY_LEN]> {
        self.inner.seed()
    }

    /// The matching verifying key.
    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        VerifyingKey {
            encoded: self.inner.public_key(),
        }
    }

    /// Signs `message`, deterministically.
    ///
    /// # Errors
    ///
    /// [`NodeError::SignatureVerification`] if the backend refuses.
    pub fn sign(&self, message: &[u8]) -> Result<[u8; SIGNATURE_LENGTH]> {
        self.inner
            .sign(message)
            .map_err(|_| NodeError::SignatureVerification)
    }
}

impl VerifyingKey {
    /// Wraps an encoded public key. Any 2592 bytes decode under FIPS 204; a
    /// key that is not some validator's simply verifies nothing.
    ///
    /// # Errors
    ///
    /// Never today; `Result` keeps the signature of [`crate::crypto::keys`].
    pub fn from_bytes(bytes: &[u8; PUBLIC_KEY_LEN]) -> Result<Self> {
        Ok(Self { encoded: *bytes })
    }

    /// The encoding.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; PUBLIC_KEY_LEN] {
        self.encoded
    }

    /// Verifies `signature` over `message`.
    ///
    /// # Errors
    ///
    /// [`NodeError::SignatureVerification`] if it does not verify.
    pub fn verify(&self, message: &[u8], signature: &[u8; SIGNATURE_LENGTH]) -> Result<()> {
        if backend::verify(&self.encoded, message, signature) {
            Ok(())
        } else {
            Err(NodeError::SignatureVerification)
        }
    }
}

/// The key a 32-byte seed names. For tests and devnets: a real validator key
/// comes from [`generate_signing_key`] and is sealed at rest.
///
/// # Errors
///
/// Never today; kept fallible to match [`crate::crypto::keys`].
pub fn signing_key_from_seed(seed: &[u8; 32]) -> Result<SigningKey> {
    SigningKey::from_bytes(seed)
}

/// A fresh key from the operating system's generator.
///
/// # Errors
///
/// [`NodeError::Network`] if the entropy source is unavailable. Fatal: a key
/// must never come from degraded randomness.
pub fn generate_signing_key() -> Result<SigningKey> {
    ValidatorSigningKey::generate()
        .map(|inner| SigningKey { inner })
        .map_err(|e| NodeError::Network(format!("OS entropy source unavailable: {e}")))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn a_validator_signature_verifies_and_round_trips() {
        let key = signing_key_from_seed(&[5; 32]).unwrap();
        let sig = key.sign(b"vote").unwrap();
        key.verifying_key().verify(b"vote", &sig).unwrap();
        assert!(key.verifying_key().verify(b"v0te", &sig).is_err());
        let back = SigningKey::from_bytes(&key.to_bytes()).unwrap();
        assert_eq!(back.verifying_key(), key.verifying_key());
    }

    #[test]
    fn a_transaction_key_signature_is_not_a_validator_signature() {
        // Different parameter set and context: the bytes cannot even line up,
        // and the type system keeps the two keys apart besides.
        let tx = crate::crypto::keys::signing_key_from_seed(&[5; 32]).unwrap();
        let tx_sig = tx.sign(b"vote").unwrap();
        assert_ne!(tx_sig.len(), SIGNATURE_LENGTH);
    }
}
