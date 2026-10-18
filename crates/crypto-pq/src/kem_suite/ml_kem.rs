//! ML-KEM-768 and ML-KEM-1024 (FIPS 203).
//!
//! Randomness comes from `getrandom` into FIPS 203's seeded entry points
//! (`KeyGen_internal(d, z)`, `Encaps_internal(m)`), so an entropy failure is
//! an error rather than a panic in the backend's RNG adapter — and the same
//! entry points are the ones the ACVP vectors exercise.

use ::ml_kem::kem::{Decapsulate as _, KeyExport as _};
use zeroize::{ZeroizeOnDrop, Zeroizing};

use super::{KemId, KemSuite, KemSuiteError, SharedSecret, check_len};

/// A decapsulation key. `ml-kem`'s own type wipes itself when its `zeroize`
/// feature is on (it is, in this crate's manifest); the wrapper exists to
/// carry the `ZeroizeOnDrop` bound the trait asks for.
pub struct MlKemKey<K>(K);

impl<K> core::fmt::Debug for MlKemKey<K> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("MlKemKey(<redacted>)")
    }
}

/// A fresh keygen seed, filled in place in the backend's array type.
///
/// Filled in place rather than in a `[u8; 64]` and then converted, so no
/// unwrapped copy of the seed is ever made on the stack in this crate.
fn random_seed() -> Result<Zeroizing<::ml_kem::Seed>, KemSuiteError> {
    let mut seed = Zeroizing::new(::ml_kem::Seed::default());
    getrandom::fill(seed.as_mut_slice()).map_err(|_| KemSuiteError::Entropy)?;
    Ok(seed)
}

/// A fresh encapsulation randomness `m`, filled in place as [`random_seed`] is.
fn random_m() -> Result<Zeroizing<::ml_kem::B32>, KemSuiteError> {
    let mut m = Zeroizing::new(::ml_kem::B32::default());
    getrandom::fill(m.as_mut_slice()).map_err(|_| KemSuiteError::Entropy)?;
    Ok(m)
}

macro_rules! ml_kem_suite {
    ($name:ident, $module:ident, $id:expr, $ek:literal, $ct:literal, $doc:literal) => {
        #[doc = $doc]
        pub struct $name;

        impl ZeroizeOnDrop for MlKemKey<::ml_kem::$module::DecapsulationKey> {}

        impl KemSuite for $name {
            const ID: KemId = $id;
            const ENCAPSULATION_KEY_LEN: usize = $ek;
            const CIPHERTEXT_LEN: usize = $ct;
            type DecapsulationKey = MlKemKey<::ml_kem::$module::DecapsulationKey>;

            fn generate() -> Result<(Self::DecapsulationKey, Vec<u8>), KemSuiteError> {
                let seed = random_seed()?;
                // The clone is moved into ml-kem and consumed there; `seed` itself wipes on drop.
                let dk = ::ml_kem::$module::DecapsulationKey::from_seed((*seed).clone());
                let ek = dk.encapsulation_key().to_bytes().to_vec();
                Ok((MlKemKey(dk), ek))
            }

            fn encapsulate(ek: &[u8]) -> Result<(Vec<u8>, SharedSecret), KemSuiteError> {
                let err = KemSuiteError::EncapsulationKey(Self::ID);
                check_len(ek.len(), $ek, err)?;
                let key = ::ml_kem::$module::EncapsulationKey::new(ek.try_into().map_err(|_| err)?)
                    .map_err(|_| err)?;
                let m = random_m()?;
                let (ct, ss) = key.encapsulate_deterministic(&m);
                let ss = Zeroizing::new(ss);
                Ok((ct.to_vec(), SharedSecret::from_slice(&ss)))
            }

            fn decapsulate(
                key: &Self::DecapsulationKey,
                ct: &[u8],
            ) -> Result<SharedSecret, KemSuiteError> {
                let err = KemSuiteError::Ciphertext(Self::ID);
                check_len(ct.len(), $ct, err)?;
                let ss = Zeroizing::new(key.0.decapsulate(ct.try_into().map_err(|_| err)?));
                Ok(SharedSecret::from_slice(&ss))
            }
        }
    };
}

ml_kem_suite!(
    MlKem768,
    ml_kem_768,
    KemId::MlKem768,
    1184,
    1088,
    "ML-KEM-768 (FIPS 203), NIST category 3."
);
ml_kem_suite!(
    MlKem1024,
    ml_kem_1024,
    KemId::MlKem1024,
    1568,
    1568,
    "ML-KEM-1024 (FIPS 203), NIST category 5."
);
