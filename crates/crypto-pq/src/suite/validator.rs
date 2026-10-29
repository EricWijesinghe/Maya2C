//! ML-DSA-87 validator keys (ADR-051 option 3, approved 2026-10-28).
//!
//! The same backend as suite `0x11` (invariant 4: ML-DSA-87 comes through
//! `ml-dsa`, never `fips204`), with one difference: a non-empty FIPS 204
//! context. Suite keys sign with an empty context and put the domain in the
//! message; a validator signature also carries `maya2c validator v1`, so it can
//! never verify as a suite signature over the same bytes.
//!
//! The key is held as its seed `ξ`: 32 secret bytes to seal and wipe, with the
//! expanded key recomputed when needed.

use ml_dsa::{EncodedVerifyingKey, MlDsa87, Signature, VerifyingKey};
use zeroize::Zeroizing;

use super::ml_dsa::{ML_DSA_87_PUBLIC_KEY_LEN, ML_DSA_87_SIGNATURE_LEN, key_from_xi};
use super::{SuiteError, SuiteId};

/// Encoded public key length.
pub const PUBLIC_KEY_LEN: usize = ML_DSA_87_PUBLIC_KEY_LEN;
/// Encoded signature length.
pub const SIGNATURE_LEN: usize = ML_DSA_87_SIGNATURE_LEN;
/// Secret length: the seed `ξ`.
pub const SEED_LEN: usize = 32;

/// The FIPS 204 context every validator signature is bound to.
pub const CONTEXT: &[u8] = b"maya2c validator v1";

/// An ML-DSA-87 validator signing key. Both fields wipe on drop.
pub struct ValidatorSigningKey {
    xi: Zeroizing<[u8; SEED_LEN]>,
    key: ml_dsa::SigningKey<MlDsa87>,
}

impl Clone for ValidatorSigningKey {
    fn clone(&self) -> Self {
        Self::from_seed(&self.xi)
    }
}

impl ValidatorSigningKey {
    /// The key a seed names: FIPS 204 `ML-DSA.KeyGen_internal`.
    #[must_use]
    pub fn from_seed(xi: &[u8; SEED_LEN]) -> Self {
        Self {
            xi: Zeroizing::new(*xi),
            key: key_from_xi::<MlDsa87>(xi),
        }
    }

    /// A fresh key from the operating system's generator.
    ///
    /// # Errors
    ///
    /// [`SuiteError::Entropy`] if the generator fails. Fatal, never retried
    /// with a weaker source.
    pub fn generate() -> Result<Self, SuiteError> {
        let mut xi = Zeroizing::new([0u8; SEED_LEN]);
        getrandom::fill(xi.as_mut()).map_err(|_| SuiteError::Entropy)?;
        Ok(Self::from_seed(&xi))
    }

    /// The seed, for the sealed keystore. Wiped when the caller drops it.
    #[must_use]
    pub fn seed(&self) -> Zeroizing<[u8; SEED_LEN]> {
        self.xi.clone()
    }

    /// The encoded public key.
    #[must_use]
    pub fn public_key(&self) -> [u8; PUBLIC_KEY_LEN] {
        self.key.expanded_key().verifying_key().encode().into()
    }

    /// Signs `message`, deterministically, under [`CONTEXT`].
    ///
    /// # Errors
    ///
    /// [`SuiteError::Signing`] if the backend refuses.
    pub fn sign(&self, message: &[u8]) -> Result<[u8; SIGNATURE_LEN], SuiteError> {
        self.key
            .expanded_key()
            .sign_deterministic(message, CONTEXT)
            .map(|sig| sig.encode().into())
            .map_err(|_| SuiteError::Signing(SuiteId::MlDsa87))
    }
}

/// Whether `signature` is a validator signature by `public_key` over `message`.
/// `false` for a public key that does not decode.
#[must_use]
pub fn verify(
    public_key: &[u8; PUBLIC_KEY_LEN],
    message: &[u8],
    signature: &[u8; SIGNATURE_LEN],
) -> bool {
    let Ok(encoded) = EncodedVerifyingKey::<MlDsa87>::try_from(&public_key[..]) else {
        return false;
    };
    let Ok(sig) = Signature::<MlDsa87>::try_from(&signature[..]) else {
        return false;
    };
    VerifyingKey::<MlDsa87>::decode(&encoded).verify_with_context(message, CONTEXT, &sig)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::suite::SignatureSuite;
    use crate::suite::ml_dsa::verify_with;

    #[test]
    fn a_signature_verifies_only_under_its_key_message_and_context() {
        let key = ValidatorSigningKey::from_seed(&[7; 32]);
        let other = ValidatorSigningKey::from_seed(&[8; 32]);
        let sig = key.sign(b"vote").unwrap();
        assert!(verify(&key.public_key(), b"vote", &sig));
        assert!(!verify(&key.public_key(), b"vot3", &sig));
        assert!(!verify(&other.public_key(), b"vote", &sig));
        // The empty context suites sign under does not accept it.
        assert!(!verify_with::<MlDsa87>(
            &key.public_key(),
            b"vote",
            b"",
            &sig
        ));
    }

    #[test]
    fn a_suite_0x11_signature_is_not_a_validator_signature() {
        let seed = crate::suite::MasterSeed::from_bytes([3; 32]);
        let suite_key = crate::suite::MlDsa87::signing_key_from_seed(&seed);
        let public: [u8; PUBLIC_KEY_LEN] = crate::suite::MlDsa87::public_key(&suite_key)
            .try_into()
            .unwrap();
        let sig: [u8; SIGNATURE_LEN] = crate::suite::MlDsa87::sign(&suite_key, b"vote")
            .unwrap()
            .try_into()
            .unwrap();
        assert!(!verify(&public, b"vote", &sig));
    }

    #[test]
    fn a_seed_names_one_key_and_signing_is_deterministic() {
        let a = ValidatorSigningKey::from_seed(&[1; 32]);
        let b = ValidatorSigningKey::from_seed(&a.seed());
        assert_eq!(a.public_key(), b.public_key());
        assert_eq!(a.sign(b"m").unwrap(), b.sign(b"m").unwrap());
        assert_ne!(
            a.public_key(),
            ValidatorSigningKey::from_seed(&[2; 32]).public_key()
        );
        let fresh = ValidatorSigningKey::generate().unwrap();
        assert!(verify(
            &fresh.public_key(),
            b"m",
            &fresh.sign(b"m").unwrap()
        ));
    }

    #[test]
    fn the_key_wipes_itself_on_drop() {
        fn requires_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
        requires_zeroize_on_drop::<ml_dsa::SigningKey<MlDsa87>>();
        requires_zeroize_on_drop::<Zeroizing<[u8; SEED_LEN]>>();
    }
}
