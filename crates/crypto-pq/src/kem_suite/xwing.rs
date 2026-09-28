//! X-Wing: ML-KEM-768 + X25519 (draft-connolly-cfrg-xwing-kem).
//!
//! The reason to carry a classical half at all: ML-KEM is new code. A bug in
//! a young implementation is a likelier failure today than a break of
//! X25519, and X-Wing's combiner is secure if *either* half is. The
//! construction, key expansion and combiner are the draft's, via the `RustCrypto`
//! crate, and `tests/kem_kat_tests.rs` runs the draft's own vectors.

use x_wing::{Decapsulate as _, KeyExport as _};
use zeroize::{ZeroizeOnDrop, Zeroizing};

use super::{KemId, KemSuite, KemSuiteError, SharedSecret, check_len};

/// An X-Wing decapsulation key: a 32-byte seed. `x-wing` wipes it in `Drop`
/// when its `zeroize` feature is on, as it is here.
pub struct XWingKey(x_wing::DecapsulationKey);

impl ZeroizeOnDrop for XWingKey {}

impl core::fmt::Debug for XWingKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("XWingKey(<redacted>)")
    }
}

fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>, KemSuiteError> {
    let mut out = Zeroizing::new([0u8; N]);
    getrandom::fill(out.as_mut()).map_err(|_| KemSuiteError::Entropy)?;
    Ok(out)
}

/// The X-Wing suite.
pub struct XWing;

impl XWing {
    /// A keypair expanded from a 32-byte seed, as the draft defines key
    /// generation: `(secret, encoded public)`. Deterministic, so an
    /// application can re-derive a prekey from a seed it already protects
    /// instead of storing another secret.
    #[must_use]
    pub fn key_from_seed(seed: &[u8; x_wing::DECAPSULATION_KEY_SIZE]) -> (XWingKey, Vec<u8>) {
        let dk = x_wing::DecapsulationKey::from(*seed);
        let ek = x_wing::Decapsulator::encapsulation_key(&dk)
            .to_bytes()
            .to_vec();
        (XWingKey(dk), ek)
    }
}

impl KemSuite for XWing {
    const ID: KemId = KemId::XWing;
    const ENCAPSULATION_KEY_LEN: usize = x_wing::ENCAPSULATION_KEY_SIZE;
    const CIPHERTEXT_LEN: usize = x_wing::CIPHERTEXT_SIZE;
    type DecapsulationKey = XWingKey;

    fn generate() -> Result<(XWingKey, Vec<u8>), KemSuiteError> {
        let seed = random::<{ x_wing::DECAPSULATION_KEY_SIZE }>()?;
        let dk = x_wing::DecapsulationKey::from(*seed);
        let ek = x_wing::Decapsulator::encapsulation_key(&dk)
            .to_bytes()
            .to_vec();
        Ok((XWingKey(dk), ek))
    }

    fn encapsulate(ek: &[u8]) -> Result<(Vec<u8>, SharedSecret), KemSuiteError> {
        let err = KemSuiteError::EncapsulationKey(Self::ID);
        check_len(ek.len(), Self::ENCAPSULATION_KEY_LEN, err)?;
        let key = x_wing::EncapsulationKey::try_from(ek).map_err(|_| err)?;
        let randomness = random::<{ x_wing::ENCAPSULATION_RANDOMNESS_SIZE }>()?;
        let (ct, ss) = key.encapsulate_deterministic(&(*randomness).into());
        Ok((ct.to_vec(), SharedSecret::from_slice(&ss)))
    }

    fn decapsulate(key: &XWingKey, ct: &[u8]) -> Result<SharedSecret, KemSuiteError> {
        let err = KemSuiteError::Ciphertext(Self::ID);
        check_len(ct.len(), Self::CIPHERTEXT_LEN, err)?;
        let ct: x_wing::Ciphertext = ct.try_into().map_err(|_| err)?;
        Ok(SharedSecret::from_slice(&key.0.decapsulate(&ct)))
    }
}
