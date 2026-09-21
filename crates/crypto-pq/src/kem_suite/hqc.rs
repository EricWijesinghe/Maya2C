//! HQC-128 and HQC-256 — **draft standard**.
//!
//! NIST selected HQC in March 2025 as the code-based backup to ML-KEM; FIPS
//! 207 is not final. The backend is pinned to a release candidate that
//! matches the reference implementation's KATs (`tests/kem_kat_tests.rs`),
//! and every name here says *draft* so nobody reads it as a finished standard.
//!
//! **Not constant-time.** `benches/dudect.rs` measured decapsulation of a
//! valid versus a random ciphertext at |t| = 26.9 (null control 2.5): a
//! remote attacker who can time decapsulation of chosen ciphertexts has a
//! side channel. ADR-009's addendum records it; the `hqc` feature that gates
//! this module is off by default and nothing on a transport path enables it.
//!
//! Randomness is drawn from the OS through `getrandom` and fed to the
//! backend's deterministic entry points, so an entropy failure is an error
//! the caller sees rather than a panic inside the backend's RNG adapter.

use hqc_kem::{Ciphertext, DecapsulationKey, EncapsulationKey, HqcKem, HqcParams};
use zeroize::{ZeroizeOnDrop, Zeroizing};

use super::{KemId, KemSuite, KemSuiteError, SharedSecret, check_len};

/// Salt length `hqc-kem`'s deterministic encapsulation takes.
const SALT_LEN: usize = 16;

/// An HQC decapsulation key. `hqc-kem` zeroizes its bytes in `Drop`
/// unconditionally (`types.rs`, "Zeroize + Drop (secret types)").
pub struct HqcKey<P: HqcParams>(DecapsulationKey<P>);

impl<P: HqcParams> ZeroizeOnDrop for HqcKey<P> {}

impl<P: HqcParams> core::fmt::Debug for HqcKey<P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("HqcKey(<redacted>)")
    }
}

fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>, KemSuiteError> {
    let mut out = Zeroizing::new([0u8; N]);
    getrandom::fill(out.as_mut()).map_err(|_| KemSuiteError::Entropy)?;
    Ok(out)
}

fn generate<P: HqcParams>() -> Result<(HqcKey<P>, Vec<u8>), KemSuiteError> {
    let seed = random::<32>()?;
    let (ek, dk) = HqcKem::<P>::generate_key_deterministic(&seed);
    Ok((HqcKey(dk), ek.as_ref().to_vec()))
}

fn encapsulate<P: HqcParams>(
    id: KemId,
    ek: &[u8],
) -> Result<(Vec<u8>, SharedSecret), KemSuiteError> {
    let key =
        EncapsulationKey::<P>::try_from(ek).map_err(|_| KemSuiteError::EncapsulationKey(id))?;
    // `k` is at most 32 bytes (HQC-256); drawing 32 and using a prefix keeps
    // the buffer a fixed, zeroized array.
    let message = random::<32>()?;
    let salt = random::<SALT_LEN>()?;
    let k = P::params().k;
    let (ct, ss) = key
        .encapsulate_deterministic(&message[..k], &salt)
        .map_err(|_| KemSuiteError::EncapsulationKey(id))?;
    Ok((ct.as_ref().to_vec(), SharedSecret::from_slice(ss.as_ref())))
}

fn decapsulate<P: HqcParams>(
    id: KemId,
    key: &HqcKey<P>,
    ct: &[u8],
) -> Result<SharedSecret, KemSuiteError> {
    let ct = Ciphertext::<P>::try_from(ct).map_err(|_| KemSuiteError::Ciphertext(id))?;
    Ok(SharedSecret::from_slice(key.0.decapsulate(&ct).as_ref()))
}

macro_rules! hqc_suite {
    ($name:ident, $params:ty, $id:expr, $ek:literal, $ct:literal, $doc:literal) => {
        #[doc = $doc]
        pub struct $name;

        impl KemSuite for $name {
            const ID: KemId = $id;
            const ENCAPSULATION_KEY_LEN: usize = $ek;
            const CIPHERTEXT_LEN: usize = $ct;
            type DecapsulationKey = HqcKey<$params>;

            fn generate() -> Result<(Self::DecapsulationKey, Vec<u8>), KemSuiteError> {
                generate::<$params>()
            }

            fn encapsulate(ek: &[u8]) -> Result<(Vec<u8>, SharedSecret), KemSuiteError> {
                check_len(ek.len(), $ek, KemSuiteError::EncapsulationKey(Self::ID))?;
                encapsulate::<$params>(Self::ID, ek)
            }

            fn decapsulate(
                key: &Self::DecapsulationKey,
                ct: &[u8],
            ) -> Result<SharedSecret, KemSuiteError> {
                check_len(ct.len(), $ct, KemSuiteError::Ciphertext(Self::ID))?;
                decapsulate::<$params>(Self::ID, key, ct)
            }
        }
    };
}

hqc_suite!(
    Hqc128,
    hqc_kem::Hqc128Params,
    KemId::Hqc128,
    2241,
    4433,
    "HQC-128 — draft standard (FIPS 207 not final), NIST category 1."
);
hqc_suite!(
    Hqc256,
    hqc_kem::Hqc256Params,
    KemId::Hqc256,
    7237,
    14_421,
    "HQC-256 — draft standard (FIPS 207 not final), NIST category 5."
);
