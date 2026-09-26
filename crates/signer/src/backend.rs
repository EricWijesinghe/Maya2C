//! Where the signing key lives (ADR-022).

use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite};

/// Backend failures.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// The backend is not available in this build, with the reason.
    #[error("signing backend unavailable: {0}")]
    Unavailable(&'static str),
    /// The backend failed to sign.
    #[error("signing failed")]
    Sign,
}

/// A place a validator key can sign from.
pub trait SignerBackend {
    /// The ML-DSA-65 public key.
    fn public_key(&self) -> &[u8];
    /// Signs `message` with ML-DSA-65 (FIPS 204, deterministic).
    ///
    /// # Errors
    ///
    /// [`BackendError`] if the backend cannot sign.
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, BackendError>;
}

/// The encrypted-keystore backend: the key is in this process's memory,
/// decrypted from `keystore` at start-up and zeroized on drop.
pub struct KeystoreBackend {
    key: <MlDsa65 as SignatureSuite>::SigningKey,
    public: Vec<u8>,
}

impl KeystoreBackend {
    /// A backend holding the key derived from `seed`.
    #[must_use]
    pub fn new(seed: &MasterSeed) -> Self {
        let key = MlDsa65::signing_key_from_seed(seed);
        let public = MlDsa65::public_key(&key);
        Self { key, public }
    }
}

impl SignerBackend for KeystoreBackend {
    fn public_key(&self) -> &[u8] {
        &self.public
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, BackendError> {
        MlDsa65::sign(&self.key, message).map_err(|_| BackendError::Sign)
    }
}

/// PKCS#11 HSM backend: not built. See ADR-022 for which vendors were found
/// to offer ML-DSA and why none was integrated here.
///
/// # Errors
///
/// Always [`BackendError::Unavailable`].
pub fn pkcs11(_module: &str, _slot: u64) -> Result<Box<dyn SignerBackend>, BackendError> {
    Err(BackendError::Unavailable(
        "PKCS#11 backend not built: no ML-DSA-capable module was available to integrate and test against (ADR-022)",
    ))
}

/// Cloud KMS backend: not built (ADR-022).
///
/// # Errors
///
/// Always [`BackendError::Unavailable`].
pub fn kms(_key_uri: &str) -> Result<Box<dyn SignerBackend>, BackendError> {
    Err(BackendError::Unavailable(
        "KMS backend not built: needs cloud credentials and a paid key, which Standing Order 6 reserves for an explicit approval (ADR-022)",
    ))
}
