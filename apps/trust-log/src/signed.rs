//! A signed record: a body, its domain-separated digest, and an ML-DSA-87
//! signature over that digest.
//!
//! The digest is what gets anchored on chain, so "this record existed by
//! height H" and "the foundation signed it" are statements about one value.
//! It is `BLAKE3-derive-key(domain, JSON(body))`: the body is re-serialised
//! from its parsed struct on verification, so field order is the struct's,
//! not the file's, and a record of one kind cannot be replayed as another.

use maya_crypto_pq::suite::{MlDsa87, SignatureSuite};
use serde::{Deserialize, Serialize};

use crate::key::SignerKey;
use crate::{Error, Result, hex_array};

/// The suite label every envelope carries.
pub const SUITE: &str = "ml-dsa-87";

/// A body with its digest and signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signed<T> {
    /// What is being stated.
    pub body: T,
    /// Hex digest of the body under the record's domain.
    pub digest: String,
    /// Always [`SUITE`].
    pub suite: String,
    /// Hex ML-DSA-87 public key of the signer.
    pub public_key: String,
    /// Hex signature over the digest bytes.
    pub signature: String,
}

/// The digest of `body` under `domain`.
///
/// # Errors
///
/// [`Error::Malformed`] if the body does not serialise.
pub fn digest<T: Serialize>(domain: &str, body: &T) -> Result<[u8; 32]> {
    let bytes = serde_json::to_vec(body).map_err(|e| Error::Malformed(e.to_string()))?;
    Ok(blake3::derive_key(domain, &bytes))
}

/// Signs `body` under `domain`.
///
/// # Errors
///
/// As [`digest`] and [`SignerKey::sign`].
pub fn seal<T: Serialize>(key: &SignerKey, domain: &str, body: T) -> Result<Signed<T>> {
    let d = digest(domain, &body)?;
    let signature = key.sign(&d)?;
    Ok(Signed {
        body,
        digest: hex::encode(d),
        suite: SUITE.to_owned(),
        public_key: hex::encode(key.public_key()),
        signature: hex::encode(signature),
    })
}

impl<T: Serialize> Signed<T> {
    /// Checks the record against the pinned signer key and returns its digest.
    ///
    /// The key is pinned by the caller, never taken from the record: a record
    /// that brings its own key proves only that somebody signed it.
    ///
    /// # Errors
    ///
    /// [`Error::Signature`] for a wrong suite, a key other than `pinned`, a
    /// digest that does not match the body, or a bad signature.
    pub fn open(&self, domain: &str, pinned: &[u8]) -> Result<[u8; 32]> {
        if self.suite != SUITE {
            return Err(Error::Signature(format!(
                "suite {:?}, want {SUITE}",
                self.suite
            )));
        }
        let key = hex::decode(&self.public_key)
            .map_err(|e| Error::Malformed(format!("public_key: {e}")))?;
        if key != pinned {
            return Err(Error::Signature(
                "signed by a key other than the pinned one".into(),
            ));
        }
        let d = digest(domain, &self.body)?;
        if hex_array::<32>("digest", &self.digest)? != d {
            return Err(Error::Signature("digest does not match the body".into()));
        }
        let signature = hex::decode(&self.signature)
            .map_err(|e| Error::Malformed(format!("signature: {e}")))?;
        MlDsa87::verify(pinned, &d, &signature).map_err(|e| Error::Signature(e.to_string()))?;
        Ok(d)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use zeroize::Zeroizing;

    const DOMAIN: &str = "maya2c trust-log test v1";

    pub(crate) fn key(byte: u8) -> SignerKey {
        SignerKey::from_seed(&Zeroizing::new([byte; 32]))
    }

    #[test]
    fn sealed_record_opens_under_its_key() {
        let k = key(1);
        let s = seal(&k, DOMAIN, "hello".to_owned()).unwrap();
        assert_eq!(
            s.open(DOMAIN, k.public_key()).unwrap(),
            digest(DOMAIN, &"hello").unwrap()
        );
    }

    #[test]
    fn edited_body_is_refused() {
        let k = key(1);
        let mut s = seal(&k, DOMAIN, "hello".to_owned()).unwrap();
        s.body = "hellO".into();
        assert!(matches!(
            s.open(DOMAIN, k.public_key()),
            Err(Error::Signature(_))
        ));
    }

    #[test]
    fn edited_body_with_recomputed_digest_fails_the_signature() {
        let k = key(1);
        let mut s = seal(&k, DOMAIN, "hello".to_owned()).unwrap();
        s.body = "hellO".into();
        s.digest = hex::encode(digest(DOMAIN, &s.body).unwrap());
        assert!(matches!(
            s.open(DOMAIN, k.public_key()),
            Err(Error::Signature(_))
        ));
    }

    #[test]
    fn a_record_carrying_another_key_is_refused() {
        let (k, other) = (key(1), key(2));
        let s = seal(&other, DOMAIN, "hello".to_owned()).unwrap();
        assert!(matches!(
            s.open(DOMAIN, k.public_key()),
            Err(Error::Signature(_))
        ));
    }

    #[test]
    fn a_record_of_another_domain_is_refused() {
        let k = key(1);
        let s = seal(&k, DOMAIN, "hello".to_owned()).unwrap();
        assert!(s.open("maya2c trust-log other v1", k.public_key()).is_err());
    }
}
