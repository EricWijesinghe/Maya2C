//! Suites `0x10` and `0x11`: ML-DSA-65 and ML-DSA-87 (FIPS 204).
//!
//! **Deterministic signing, empty context.** FIPS 204 permits both hedged and
//! deterministic signing; the node has always signed deterministically
//! (`custom_l1_node::crypto::keys`), and a suite that signed differently would
//! make the `0x30` hybrid's lattice half disagree with the node byte for byte.
//! Domain separation lives inside the signed message, where the caller puts
//! it, so the FIPS context string is empty — a second place to put a domain is
//! a second encoding of the same intent that also verifies.
//!
//! Keys are held in seed form (`ξ`, 32 bytes). The expanded key is recomputed
//! from it, which costs a few microseconds and means only 32 secret bytes ever
//! need wiping.

use ml_dsa::{EncodedSignature, EncodedVerifyingKey, MlDsaParams, Signature, VerifyingKey};

use super::{MasterSeed, SignatureSuite, SuiteError, SuiteId};

/// ML-DSA-65 public key length (FIPS 204 Table 2).
pub const ML_DSA_65_PUBLIC_KEY_LEN: usize = 1952;
/// ML-DSA-65 signature length (FIPS 204 Table 2, final — not the draft 3,293).
pub const ML_DSA_65_SIGNATURE_LEN: usize = 3309;
/// ML-DSA-87 public key length.
pub const ML_DSA_87_PUBLIC_KEY_LEN: usize = 2592;
/// ML-DSA-87 signature length.
pub const ML_DSA_87_SIGNATURE_LEN: usize = 4627;

const _: () = {
    use core::mem::size_of;
    assert!(size_of::<EncodedVerifyingKey<ml_dsa::MlDsa65>>() == ML_DSA_65_PUBLIC_KEY_LEN);
    assert!(size_of::<EncodedSignature<ml_dsa::MlDsa65>>() == ML_DSA_65_SIGNATURE_LEN);
    assert!(size_of::<EncodedVerifyingKey<ml_dsa::MlDsa87>>() == ML_DSA_87_PUBLIC_KEY_LEN);
    assert!(size_of::<EncodedSignature<ml_dsa::MlDsa87>>() == ML_DSA_87_SIGNATURE_LEN);
};

/// The FIPS 204 context string. Empty on purpose; see the module docs.
const CONTEXT: &[u8] = b"";

/// Builds the key from `ξ`: FIPS 204 `ML-DSA.KeyGen_internal`.
pub(crate) fn key_from_xi<P: MlDsaParams>(xi: &[u8; 32]) -> ml_dsa::SigningKey<P> {
    ml_dsa::SigningKey::<P>::from_seed(&(*xi).into())
}

pub(crate) fn encode_public<P: MlDsaParams>(key: &ml_dsa::SigningKey<P>) -> Vec<u8> {
    key.expanded_key().verifying_key().encode().to_vec()
}

pub(crate) fn sign_with<P: MlDsaParams>(
    id: SuiteId,
    key: &ml_dsa::SigningKey<P>,
    message: &[u8],
) -> Result<Vec<u8>, SuiteError> {
    key.expanded_key()
        .sign_deterministic(message, CONTEXT)
        .map(|sig| sig.encode().to_vec())
        .map_err(|_| SuiteError::Signing(id))
}

/// Length-checked verification. Returns `false` rather than an error for a
/// bad signature so the hybrid can combine halves without branching early.
pub(crate) fn verify_with<P: MlDsaParams>(
    public_key: &[u8],
    message: &[u8],
    context: &[u8],
    signature: &[u8],
) -> bool {
    let Ok(encoded) = EncodedVerifyingKey::<P>::try_from(public_key) else {
        return false;
    };
    let Ok(sig) = Signature::<P>::try_from(signature) else {
        return false;
    };
    VerifyingKey::<P>::decode(&encoded).verify_with_context(message, context, &sig)
}

macro_rules! ml_dsa_suite {
    ($name:ident, $params:ty, $id:expr, $domain:literal, $doc:literal) => {
        #[doc = $doc]
        pub struct $name;

        impl SignatureSuite for $name {
            const ID: SuiteId = $id;
            type SigningKey = ml_dsa::SigningKey<$params>;

            fn signing_key_from_seed(seed: &MasterSeed) -> Self::SigningKey {
                key_from_xi::<$params>(&super::expand::<32>($domain, seed))
            }

            fn public_key(key: &Self::SigningKey) -> Vec<u8> {
                encode_public(key)
            }

            fn sign(key: &Self::SigningKey, message: &[u8]) -> Result<Vec<u8>, SuiteError> {
                sign_with(Self::ID, key, message)
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

ml_dsa_suite!(
    MlDsa65,
    ml_dsa::MlDsa65,
    SuiteId::MlDsa65,
    "maya2c 2026-09-21 suite 0x10 ml-dsa-65 xi v1",
    "Suite `0x10`: ML-DSA-65, NIST category 3."
);
ml_dsa_suite!(
    MlDsa87,
    ml_dsa::MlDsa87,
    SuiteId::MlDsa87,
    "maya2c 2026-09-21 suite 0x11 ml-dsa-87 xi v1",
    "Suite `0x11`: ML-DSA-87, NIST category 5 — the mainnet default."
);
