//! Suites `0x20` and `0x21`: SLH-DSA-SHA2-128s and SLH-DSA-SHAKE-256f
//! (FIPS 205).
//!
//! Stateless hash-based signatures: their security rests on the hash
//! function alone, which is why they are the conservative half of the hybrid
//! and the cold-vault suite. `128s` is small and slow to sign; `256f` is
//! category 5 and fast to sign, at 49,856 bytes a signature — acceptable for a
//! vault that signs rarely, unacceptable for a payment.
//!
//! Signing is deterministic (`opt_rand` absent) with an empty context, for
//! the same reasons as ML-DSA: see `ml_dsa.rs`.
//!
//! This is the crate that instantiates `slh-dsa` (invariant 2), and these are
//! concrete types, so the monomorphized code lands here, where the dev
//! profile optimizes it.

use slh_dsa::signature::Keypair as _;
use slh_dsa::{ParameterSet, Sha2_128s, Shake256f, Signature, SigningKey, VerifyingKey};

use super::{MasterSeed, SignatureSuite, SuiteError, SuiteId};

/// SLH-DSA-SHA2-128s public key length (FIPS 205 Table 2).
pub const SHA2_128S_PUBLIC_KEY_LEN: usize = 32;
/// SLH-DSA-SHA2-128s signature length.
pub const SHA2_128S_SIGNATURE_LEN: usize = 7856;
/// SLH-DSA-SHAKE-256f public key length.
pub const SHAKE_256F_PUBLIC_KEY_LEN: usize = 64;
/// SLH-DSA-SHAKE-256f signature length.
pub const SHAKE_256F_SIGNATURE_LEN: usize = 49_856;

const CONTEXT: &[u8] = b"";

/// FIPS 205 `slh_keygen_internal` from three `n`-byte seeds drawn from `material`.
fn keygen<P: ParameterSet>(material: &[u8]) -> SigningKey<P> {
    let n = material.len() / 3;
    SigningKey::<P>::slh_keygen_internal(&material[..n], &material[n..2 * n], &material[2 * n..])
}

fn sign<P: ParameterSet>(
    id: SuiteId,
    key: &SigningKey<P>,
    message: &[u8],
) -> Result<Vec<u8>, SuiteError> {
    key.try_sign_with_context(message, CONTEXT, None)
        .map(|sig| sig.to_vec())
        .map_err(|_| SuiteError::Signing(id))
}

/// `false` for anything that does not verify, including malformed encodings.
pub(crate) fn verify_with<P: ParameterSet>(
    public_key: &[u8],
    message: &[u8],
    context: &[u8],
    signature: &[u8],
) -> bool {
    let Ok(key) = VerifyingKey::<P>::try_from(public_key) else {
        return false;
    };
    let Ok(sig) = Signature::<P>::try_from(signature) else {
        return false;
    };
    key.try_verify_with_context(message, context, &sig).is_ok()
}

macro_rules! slh_suite {
    ($name:ident, $params:ty, $id:expr, $n:literal, $domain:literal, $doc:literal) => {
        #[doc = $doc]
        pub struct $name;

        impl SignatureSuite for $name {
            const ID: SuiteId = $id;
            type SigningKey = SigningKey<$params>;

            fn signing_key_from_seed(seed: &MasterSeed) -> Self::SigningKey {
                keygen::<$params>(super::expand::<{ 3 * $n }>($domain, seed).as_slice())
            }

            fn public_key(key: &Self::SigningKey) -> Vec<u8> {
                key.verifying_key().to_vec()
            }

            fn sign(key: &Self::SigningKey, message: &[u8]) -> Result<Vec<u8>, SuiteError> {
                sign(Self::ID, key, message)
            }

            fn verify(
                public_key: &[u8],
                message: &[u8],
                signature: &[u8],
            ) -> Result<(), SuiteError> {
                super::check_public_key_len(Self::ID, public_key)?;
                super::check_signature_len(Self::ID, signature)?;
                if verify_with::<$params>(public_key, message, CONTEXT, signature) {
                    Ok(())
                } else {
                    Err(SuiteError::Verification(Self::ID))
                }
            }
        }
    };
}

slh_suite!(
    SlhDsaSha2_128s,
    Sha2_128s,
    SuiteId::SlhDsaSha2_128s,
    16,
    "maya2c 2026-09-21 suite 0x20 slh-dsa-sha2-128s seeds v1",
    "Suite `0x20`: SLH-DSA-SHA2-128s, NIST category 1."
);
slh_suite!(
    SlhDsaShake256f,
    Shake256f,
    SuiteId::SlhDsaShake256f,
    32,
    "maya2c 2026-09-21 suite 0x21 slh-dsa-shake-256f seeds v1",
    "Suite `0x21`: SLH-DSA-SHAKE-256f, NIST category 5 — cold vaults and archive seals."
);
