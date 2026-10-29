//! Where the signing key lives (ADR-022).

use maya_crypto_pq::suite::{MasterSeed, ValidatorSigningKey};

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
    /// The ML-DSA-87 validator public key (ADR-051 option 3).
    fn public_key(&self) -> &[u8];
    /// Signs `message` as a validator: ML-DSA-87, deterministic, under the
    /// validator context (`maya_crypto_pq::suite::validator::CONTEXT`).
    ///
    /// # Errors
    ///
    /// [`BackendError`] if the backend cannot sign.
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, BackendError>;
}

/// The encrypted-keystore backend: the key is in this process's memory,
/// decrypted from `keystore` at start-up and zeroized on drop.
pub struct KeystoreBackend {
    key: ValidatorSigningKey,
    public: Vec<u8>,
}

impl KeystoreBackend {
    /// A backend holding the validator key `seed` names (the seed is `ξ`).
    #[must_use]
    pub fn new(seed: &MasterSeed) -> Self {
        let key = ValidatorSigningKey::from_seed(seed.expose());
        let public = key.public_key().to_vec();
        Self { key, public }
    }
}

impl SignerBackend for KeystoreBackend {
    fn public_key(&self) -> &[u8] {
        &self.public
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, BackendError> {
        self.key
            .sign(message)
            .map(|sig| sig.to_vec())
            .map_err(|_| BackendError::Sign)
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
