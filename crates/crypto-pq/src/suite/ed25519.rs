//! Suite `0x01`: Ed25519, for devnet and legacy data only.
//!
//! Zero post-quantum security — Shor's algorithm recovers the key from the
//! public key — so the registry marks it `mainnet_allowed: false` and the
//! audit flags it. It exists because devnet tooling and some legacy records
//! use it, and reading legacy data is the point of crypto-agility.
//!
//! **`verify_strict` only.** Plain `verify` accepts small-order public keys
//! and the mixed-order encodings that make Ed25519 signatures malleable
//! between implementations; in a consensus rule, "two correct nodes disagree
//! about validity" is a fork. `verify_strict` rejects both.

use ed25519_dalek::pkcs8::{DecodePrivateKey as _, EncodePrivateKey as _};
use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use zeroize::Zeroizing;

use super::{MasterSeed, SignatureSuite, SuiteError, SuiteId};

/// Encoded public key length.
pub const PUBLIC_KEY_LEN: usize = ed25519_dalek::PUBLIC_KEY_LENGTH;
/// Encoded signature length.
pub const SIGNATURE_LEN: usize = ed25519_dalek::SIGNATURE_LENGTH;

const _: () = {
    assert!(PUBLIC_KEY_LEN == 32);
    assert!(SIGNATURE_LEN == 64);
};

const SEED_DOMAIN: &str = "maya2c 2026-09-21 suite 0x01 ed25519 seed v1";

/// The Ed25519 suite.
pub struct Ed25519;

impl Ed25519 {
    /// Loads a signing key from a PKCS#8 v1/v2 DER document (RFC 8410).
    ///
    /// # Errors
    ///
    /// [`SuiteError::KeyEncoding`] if the document is not an Ed25519 key.
    pub fn from_pkcs8_der(der: &[u8]) -> Result<SigningKey, SuiteError> {
        SigningKey::from_pkcs8_der(der).map_err(|_| SuiteError::KeyEncoding(SuiteId::Ed25519))
    }

    /// Encodes a signing key as PKCS#8 DER, in a buffer wiped on drop.
    ///
    /// # Errors
    ///
    /// [`SuiteError::KeyEncoding`] if encoding fails.
    pub fn to_pkcs8_der(key: &SigningKey) -> Result<Zeroizing<Vec<u8>>, SuiteError> {
        key.to_pkcs8_der()
            .map(|doc| Zeroizing::new(doc.as_bytes().to_vec()))
            .map_err(|_| SuiteError::KeyEncoding(SuiteId::Ed25519))
    }
}

impl SignatureSuite for Ed25519 {
    const ID: SuiteId = SuiteId::Ed25519;
    type SigningKey = SigningKey;

    fn signing_key_from_seed(seed: &MasterSeed) -> SigningKey {
        let secret = super::expand::<32>(SEED_DOMAIN, seed);
        SigningKey::from_bytes(&secret)
    }

    fn public_key(key: &SigningKey) -> Vec<u8> {
        key.verifying_key().to_bytes().to_vec()
    }

    fn sign(key: &SigningKey, message: &[u8]) -> Result<Vec<u8>, SuiteError> {
        Ok(key.sign(message).to_bytes().to_vec())
    }

    fn verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), SuiteError> {
        super::check_public_key_len(Self::ID, public_key)?;
        super::check_signature_len(Self::ID, signature)?;
        let pk: [u8; PUBLIC_KEY_LEN] = public_key
            .try_into()
            .map_err(|_| SuiteError::MalformedPublicKey(Self::ID))?;
        let sig: [u8; SIGNATURE_LEN] = signature
            .try_into()
            .map_err(|_| SuiteError::Verification(Self::ID))?;
        let key =
            VerifyingKey::from_bytes(&pk).map_err(|_| SuiteError::MalformedPublicKey(Self::ID))?;
        key.verify_strict(message, &ed25519_dalek::Signature::from_bytes(&sig))
            .map_err(|_| SuiteError::Verification(Self::ID))
    }
}
