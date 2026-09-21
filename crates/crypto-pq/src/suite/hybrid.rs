//! Suite `0x30`: ML-DSA-65 **and** SLH-DSA-SHA2-128s.
//!
//! Valid only if both halves verify. A forger has to break Module-LWE *and*
//! the hash function, so the suite survives a cryptanalytic surprise in either
//! family alone — the reason every transaction on the chain has carried this
//! pair.
//!
//! **Byte-compatible with `custom_l1_node::crypto::hybrid`.** The public key
//! is `ml_dsa_pk ‖ slh_pk`, the signature `ml_dsa_sig ‖ slh_sig`, and keys
//! derive from a 32-byte chain key under the node's own domain strings, so an
//! existing account's key and every signature it ever made are a valid `0x30`
//! key and signature unchanged. `crates/node/tests/suite_parity_tests.rs` pins
//! that against the node's `fips204` implementation.
//!
//! **Both halves are always checked.** No early return after the first
//! failure, so the time taken does not tell a forger which half they got
//! wrong.

use zeroize::{ZeroizeOnDrop, Zeroizing};

use super::ml_dsa::{self as lattice, ML_DSA_65_PUBLIC_KEY_LEN, ML_DSA_65_SIGNATURE_LEN};
use super::slh_dsa::{SHA2_128S_PUBLIC_KEY_LEN, SHA2_128S_SIGNATURE_LEN};
use super::{MasterSeed, SignatureSuite, SuiteError, SuiteId};
use crate::sig;

/// `ml_dsa_pk ‖ slh_pk`.
pub const PUBLIC_KEY_LEN: usize = ML_DSA_65_PUBLIC_KEY_LEN + SHA2_128S_PUBLIC_KEY_LEN;
/// `ml_dsa_sig ‖ slh_sig`.
pub const SIGNATURE_LEN: usize = ML_DSA_65_SIGNATURE_LEN + SHA2_128S_SIGNATURE_LEN;

const _: () = {
    assert!(PUBLIC_KEY_LEN == 1984);
    assert!(SIGNATURE_LEN == 11_165);
    assert!(sig::SIGNATURE_LEN == SHA2_128S_SIGNATURE_LEN);
    assert!(sig::PUBLIC_KEY_LEN == SHA2_128S_PUBLIC_KEY_LEN);
};

/// The node's domain strings, verbatim. Changing either changes every
/// account's address.
const LATTICE_SEED_DOMAIN: &[u8] = b"custom-l1-node.hybrid-seed.ml-dsa-65.v1";
const HASH_SEED_DOMAIN: &[u8] = b"custom-l1-node.hybrid-seed.slh-dsa-sha2-128s.v1";

/// Both secret halves.
pub struct HybridSigningKey {
    lattice: ml_dsa::SigningKey<ml_dsa::MlDsa65>,
    hash_based: sig::SigningKey,
}

// Both fields wipe themselves on drop: `ml_dsa::SigningKey` is
// `ZeroizeOnDrop`, and `sig::SigningKey` wraps an `slh_dsa::SigningKey`,
// which is too. The marker records that for the trait bound.
impl ZeroizeOnDrop for HybridSigningKey {}

impl core::fmt::Debug for HybridSigningKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("HybridSigningKey(<redacted>)")
    }
}

/// The node's `derive_seed`: `BLAKE3(domain ‖ chain_key)`.
fn derive_seed(domain: &[u8], chain_key: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(chain_key);
    Zeroizing::new(*hasher.finalize().as_bytes())
}

/// The hybrid suite.
pub struct HybridMlDsa65SlhDsa128s;

impl SignatureSuite for HybridMlDsa65SlhDsa128s {
    const ID: SuiteId = SuiteId::HybridMlDsa65SlhDsa128s;
    type SigningKey = HybridSigningKey;

    /// The master seed *is* the node's chain key.
    fn signing_key_from_seed(seed: &MasterSeed) -> HybridSigningKey {
        let chain_key = seed.expose();
        HybridSigningKey {
            lattice: lattice::key_from_xi(&derive_seed(LATTICE_SEED_DOMAIN, chain_key)),
            hash_based: sig::signing_key_from_seed(&derive_seed(HASH_SEED_DOMAIN, chain_key)),
        }
    }

    fn public_key(key: &HybridSigningKey) -> Vec<u8> {
        let mut out = lattice::encode_public(&key.lattice);
        out.extend_from_slice(&key.hash_based.verifying_key().to_bytes());
        out
    }

    fn sign(key: &HybridSigningKey, message: &[u8]) -> Result<Vec<u8>, SuiteError> {
        let mut out = lattice::sign_with(Self::ID, &key.lattice, message)?;
        out.extend_from_slice(&key.hash_based.sign(message));
        Ok(out)
    }

    fn verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), SuiteError> {
        super::check_public_key_len(Self::ID, public_key)?;
        super::check_signature_len(Self::ID, signature)?;
        let (lattice_pk, hash_pk) = public_key.split_at(ML_DSA_65_PUBLIC_KEY_LEN);
        let (lattice_sig, hash_sig) = signature.split_at(ML_DSA_65_SIGNATURE_LEN);

        let lattice_ok =
            lattice::verify_with::<ml_dsa::MlDsa65>(lattice_pk, message, b"", lattice_sig);
        let hash_ok =
            super::slh_dsa::verify_with::<slh_dsa::Sha2_128s>(hash_pk, message, b"", hash_sig);

        // `&`, not `&&`: both halves have already run, and this keeps it so
        // if someone later inlines the calls into the condition.
        if lattice_ok & hash_ok {
            Ok(())
        } else {
            Err(SuiteError::Verification(Self::ID))
        }
    }
}
