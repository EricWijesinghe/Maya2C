//! Chat identities and prekey bundles (ADR-031).
//!
//! An identity is an ML-DSA-65 key; its address is the chain's own address
//! for that key ([`maya_crypto_pq::suite::suite_address`]), so a chat
//! identity and an on-chain account can be one key. A prekey is an X-Wing
//! (ML-KEM-768 + X25519) key derived from the identity seed and an epoch, so
//! nothing but the seed needs protecting at rest.

use maya_crypto_pq::kem_suite::{XWing, XWingKey};
use maya_crypto_pq::suite::{self, MasterSeed, MlDsa65, SignatureSuite, SuiteId};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{Address, ChatError};

const PREKEY_DOMAIN: &str = "maya-chat 2026-09-28 prekey seed v1";
const BUNDLE_DOMAIN: &[u8] = b"maya-chat prekey bundle v1";

/// The address of an ML-DSA-65 identity key: the chain's address for it.
#[must_use]
pub fn address_of(identity_key: &[u8]) -> Address {
    suite::suite_address(SuiteId::MlDsa65, identity_key)
}

/// A chat identity. The seed and the signing key are wiped on drop.
pub struct Identity {
    seed: Zeroizing<[u8; 32]>,
    key: <MlDsa65 as SignatureSuite>::SigningKey,
    public: Vec<u8>,
    address: Address,
}

impl Identity {
    /// The identity a 32-byte seed expands to.
    #[must_use]
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let seed = Zeroizing::new(seed);
        let key = MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes(*seed));
        let public = MlDsa65::public_key(&key);
        let address = address_of(&public);
        Self {
            seed,
            key,
            public,
            address,
        }
    }

    /// A fresh identity from the operating system's generator.
    ///
    /// # Errors
    ///
    /// [`ChatError::Entropy`].
    pub fn generate() -> Result<Self, ChatError> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut()).map_err(|_| ChatError::Entropy)?;
        Ok(Self::from_seed(*seed))
    }

    /// The seed, for storing the identity. Handle it as the key it is.
    #[must_use]
    pub fn seed(&self) -> &[u8; 32] {
        &self.seed
    }

    /// The public identity key.
    #[must_use]
    pub fn public_key(&self) -> &[u8] {
        &self.public
    }

    /// The identity's address.
    #[must_use]
    pub fn address(&self) -> Address {
        self.address
    }

    /// Signs `message`.
    ///
    /// # Errors
    ///
    /// [`ChatError::Signature`] if the signer refuses.
    pub fn sign(&self, message: &[u8]) -> Result<Vec<u8>, ChatError> {
        MlDsa65::sign(&self.key, message).map_err(|_| ChatError::Signature)
    }

    /// The prekey for `epoch`: `(secret, encoded public)`.
    #[must_use]
    pub fn prekey(&self, epoch: u32) -> (XWingKey, Vec<u8>) {
        let mut h = blake3::Hasher::new_derive_key(PREKEY_DOMAIN);
        h.update(self.seed.as_ref());
        h.update(&epoch.to_le_bytes());
        let seed = Zeroizing::new(*h.finalize().as_bytes());
        XWing::key_from_seed(&seed)
    }

    /// A signed bundle publishing the prekey for `epoch` until `expires_at`
    /// (seconds since the Unix epoch).
    ///
    /// # Errors
    ///
    /// [`ChatError::Signature`].
    pub fn prekey_bundle(&self, epoch: u32, expires_at: u64) -> Result<PrekeyBundle, ChatError> {
        let (_, prekey) = self.prekey(epoch);
        let mut bundle = PrekeyBundle {
            identity_key: self.public.clone(),
            epoch,
            prekey,
            expires_at,
            signature: Vec::new(),
        };
        bundle.signature = self.sign(&bundle.signing_bytes())?;
        Ok(bundle)
    }
}

/// A published prekey, signed by its identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrekeyBundle {
    /// The ML-DSA-65 identity key.
    pub identity_key: Vec<u8>,
    /// Which prekey.
    pub epoch: u32,
    /// The X-Wing encapsulation key.
    pub prekey: Vec<u8>,
    /// Unix seconds after which it must not be used.
    pub expires_at: u64,
    /// The identity's signature over the fields above.
    pub signature: Vec<u8>,
}

pub(crate) fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(bytes);
}

impl PrekeyBundle {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut out = BUNDLE_DOMAIN.to_vec();
        put(&mut out, &self.identity_key);
        out.extend_from_slice(&self.epoch.to_le_bytes());
        put(&mut out, &self.prekey);
        out.extend_from_slice(&self.expires_at.to_le_bytes());
        out
    }

    /// Checks the signature and the expiry; returns the owner's address.
    ///
    /// # Errors
    ///
    /// [`ChatError::BadSignature`] or [`ChatError::Expired`].
    pub fn verify(&self, now: u64) -> Result<Address, ChatError> {
        if now >= self.expires_at {
            return Err(ChatError::Expired);
        }
        suite::verify(
            SuiteId::MlDsa65,
            &self.identity_key,
            &self.signing_bytes(),
            &self.signature,
        )
        .map_err(|_| ChatError::BadSignature)?;
        Ok(address_of(&self.identity_key))
    }
}
