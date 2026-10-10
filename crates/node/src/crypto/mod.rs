//! Cryptographic primitives: the `ArgonBlake` hash, proof-of-work evaluation,
//! and hybrid ML-DSA-65 + SLH-DSA-SHA2-128s key handling.
//!
//! [`hybrid`] is the module callers want. [`keys`] is its lattice half and
//! `maya_crypto_pq` is its hash-based half; both are exposed because the
//! keystore and the benchmarks need to name the two schemes separately, but
//! neither authorizes anything on its own.

pub mod argon_blake;
pub mod dag;
pub mod hybrid;
pub mod keys;
pub mod lattice;
pub mod pow;
pub mod suites;

pub use argon_blake::{HASH_LEN, argon_blake_hash};
pub use dag::{
    DAG_ACTIVATION_HEIGHT, EPOCH_LENGTH, Params as DagParams,
    cache::Cache,
    dataset::Dataset,
    epoch_of, epoch_seed,
    hashimoto::{Proof, hashimoto_full, hashimoto_light},
    registry::{CacheRegistry, DagConfig},
};
pub use hybrid::{
    HYBRID_PUBLIC_KEY_LEN, HYBRID_SECRET_KEY_LEN, HYBRID_SIGNATURE_LENGTH, HybridPublicKey,
    HybridSignature, HybridSigningKey, HybridVerifyingKey, SLH_DSA_PUBLIC_KEY_LEN,
    SLH_DSA_SECRET_KEY_LEN, SLH_DSA_SIGNATURE_LENGTH, address_of, generate_signing_key,
    signing_key_from_seed,
};
pub use keys::{ADDRESS_LEN, PUBLIC_KEY_LEN, SECRET_KEY_LEN, SIGNATURE_LENGTH};
pub use pow::{leading_zero_bits, meets_target, target_from_leading_zero_bits};
