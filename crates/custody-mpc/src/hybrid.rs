//! Turning a reconstructed vault secret into a `Maya2C` signing key.
//!
//! # The fact that makes threshold custody of this chain tractable at all
//!
//! A whole `Maya2C` identity is **32 bytes**. `crypto::hybrid::signing_key_from_seed`
//! (`src/crypto/hybrid.rs:469`) takes one 32-byte chain key and derives *both*
//! halves of the hybrid pair from it, through two domain-separated BLAKE3
//! seeds. So a scheme that protects 32 bytes protects the ML-DSA-65 half and
//! the SLH-DSA-SHA2-128s half together.
//!
//! That is worth stating plainly because the obvious alternative — threshold
//! signing — has to solve each half separately, and the hash-based half has no
//! threshold construction at all. See `docs/custody-mpc.md`.
//!
//! # Four constants are duplicated from the node, deliberately
//!
//! `ADDRESS_DOMAIN`, `LATTICE_SEED_DOMAIN`, `HASH_SEED_DOMAIN` and the
//! deterministic signing seed all come from `src/crypto/`. Importing them would
//! mean depending on `custom-l1-node`, which pulls `RocksDB` into a library that
//! is meant to run inside an HSM boundary and link nothing it does not need.
//!
//! This is the same arrangement `sdk-wasm` has, for the same reason, and it
//! carries the same risk: a second implementation of address derivation that
//! drifts from the node's is indistinguishable from a correct one until
//! somebody loses money. `tests/custody_parity_tests.rs`, in the node's own
//! suite, is what makes drift a test failure — it signs here and verifies
//! there, over freshly generated vaults.

use curve25519_dalek::scalar::Scalar;
use fips204::ml_dsa_65;
use fips204::traits::{KeyGen, SerDes, Signer};
use maya_crypto_pq::sig as slh;
use zeroize::Zeroizing;

use crate::error::{CustodyError, Result};

/// Encoded length of a hybrid public key: ML-DSA-65 then SLH-DSA.
pub const HYBRID_PUBLIC_KEY_LEN: usize = 1952 + 32;

/// Encoded length of a hybrid signature: ML-DSA-65 then SLH-DSA.
pub const HYBRID_SIGNATURE_LEN: usize = 3309 + 7856;

/// Length of a `Maya2C` address.
pub const ADDRESS_LEN: usize = 32;

/// Domain separator for address derivation, from `src/crypto/hybrid.rs:111`.
///
/// A different domain yields a well-formed address that **no key can spend
/// from**. There is no error path for getting this wrong — funds simply go
/// nowhere — which is why it is pinned by a test rather than reviewed by eye.
const ADDRESS_DOMAIN: &[u8] = b"custom-l1-node.address.v3";

/// Domain separators splitting one chain key into two scheme seeds, from
/// `src/crypto/hybrid.rs:118-119`.
const LATTICE_SEED_DOMAIN: &[u8] = b"custom-l1-node.hybrid-seed.ml-dsa-65.v1";
const HASH_SEED_DOMAIN: &[u8] = b"custom-l1-node.hybrid-seed.slh-dsa-sha2-128s.v1";

/// Domain separator turning a vault secret into a chain key.
///
/// This one is **not** shared with the node, because the node has no notion of
/// a vault secret. The vault's secret lives in the Ristretto scalar field —
/// that is what makes it shareable with verifiable commitments — and a scalar
/// is not a chain key. Hashing rather than reinterpreting the bytes is what
/// keeps the two domains separate: a scalar is not uniform over 32 bytes, and
/// feeding a non-uniform value straight into key derivation is the kind of
/// shortcut that costs a few bits of security for no benefit at all.
const CHAIN_KEY_DOMAIN: &[u8] = b"maya2c.custody-mpc.vault-chain-key.v1";

/// The all-zero `rnd` input selecting FIPS 204's deterministic signing variant,
/// from `src/crypto/keys.rs:90`.
///
/// Load-bearing, not a default: a `Maya2C` transaction id hashes its own
/// signature, so a hedged signature would give one transaction two identities.
const DETERMINISTIC_SEED: [u8; 32] = [0u8; 32];

/// FIPS 204's context string, empty to match the node (`keys.rs:92-96`).
const SIGNING_CONTEXT: &[u8] = b"";

/// Derives the 32-byte chain key from a reconstructed vault secret.
#[must_use]
pub fn chain_key_from_secret(secret: &Scalar) -> Zeroizing<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(CHAIN_KEY_DOMAIN);
    hasher.update(&secret.to_bytes());
    Zeroizing::new(*hasher.finalize().as_bytes())
}

/// Splits one chain key into a scheme-specific seed.
///
/// Byte-identical to the node's `derive_seed` (`src/crypto/hybrid.rs:498`): the
/// domain is **prefixed into a plain hasher**, not passed to
/// `new_derive_key`. Those produce different digests from the same input, and
/// the difference is an address nothing can spend from, reported by nothing.
/// The Ledger spike made exactly this mistake — see `docs/ledger-feasibility.md`.
fn derive_seed(domain: &[u8], chain_key: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(chain_key);
    Zeroizing::new(*hasher.finalize().as_bytes())
}

/// A vault's hybrid signing key, materialised for the length of one signature.
///
/// # This type is the trust boundary
///
/// While one of these exists, one machine can sign anything the vault owns. It
/// is created inside [`crate::session::SigningSession::sign`], used once, and
/// dropped — and its two halves zeroize on drop, `fips204`'s by
/// `ZeroizeOnDrop` and the hash-based one because `slh_dsa` does the same. That
/// bounds the window; it does not remove it. Nothing in this crate pretends
/// otherwise, and `docs/custody-mpc.md` says so in the first paragraph.
pub struct VaultKey {
    lattice: ml_dsa_65::PrivateKey,
    hash_based: slh::SigningKey,
    public_key: [u8; HYBRID_PUBLIC_KEY_LEN],
}

impl core::fmt::Debug for VaultKey {
    /// Prints the address and nothing else.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VaultKey")
            .field("address", &hex32(&self.address()))
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl VaultKey {
    /// Derives the hybrid key from a chain key.
    ///
    /// # Errors
    ///
    /// [`CustodyError::Lattice`] if ML-DSA key generation fails, which for a
    /// fixed seed indicates a defect rather than a transient condition.
    pub fn from_chain_key(chain_key: &[u8; 32]) -> Result<Self> {
        let lattice_seed = derive_seed(LATTICE_SEED_DOMAIN, chain_key);
        let hash_seed = derive_seed(HASH_SEED_DOMAIN, chain_key);

        // `keygen_from_seed` is FIPS 204's own `KeyGen_internal(ξ)`. The node
        // reaches it through a one-shot RNG (`keys.rs:346`) because that is the
        // only door its `fips204` version opened; both call the same function
        // with the same ξ, and `tests/custody_parity_tests.rs` pins that they
        // still agree.
        let (lattice_public, lattice) = ml_dsa_65::KG::keygen_from_seed(&lattice_seed);
        let hash_based = slh::signing_key_from_seed(&hash_seed);

        let mut public_key = [0u8; HYBRID_PUBLIC_KEY_LEN];
        public_key[..1952].copy_from_slice(&lattice_public.into_bytes());
        public_key[1952..].copy_from_slice(&hash_based.verifying_key().to_bytes());

        Ok(Self {
            lattice,
            hash_based,
            public_key,
        })
    }

    /// The encoded hybrid public key: lattice then hash-based, no separators.
    ///
    /// The concatenation is bare because the node's `encode_into`
    /// (`src/crypto/hybrid.rs:188`) is bare — both halves are fixed-width, so
    /// there is nothing to delimit.
    #[must_use]
    pub fn public_key(&self) -> [u8; HYBRID_PUBLIC_KEY_LEN] {
        self.public_key
    }

    /// The vault's address: `BLAKE3(ADDRESS_DOMAIN || lattice || hash_based)`.
    #[must_use]
    pub fn address(&self) -> [u8; ADDRESS_LEN] {
        address_of(&self.public_key)
    }

    /// Signs `message`, producing both halves.
    ///
    /// # Errors
    ///
    /// [`CustodyError::Lattice`] if ML-DSA's rejection loop fails to terminate,
    /// which FIPS 204 permits an implementation to report and which no caller
    /// can recover from. The hash-based half has no rejection loop and cannot
    /// fail.
    pub fn sign(&self, message: &[u8]) -> Result<[u8; HYBRID_SIGNATURE_LEN]> {
        let lattice = self
            .lattice
            .try_sign_with_seed(&DETERMINISTIC_SEED, message, SIGNING_CONTEXT)
            .map_err(CustodyError::Lattice)?;
        let hash_based = self.hash_based.sign(message);

        let mut out = [0u8; HYBRID_SIGNATURE_LEN];
        out[..3309].copy_from_slice(&lattice);
        out[3309..].copy_from_slice(&hash_based);
        Ok(out)
    }
}

/// Derives an address from an encoded hybrid public key.
#[must_use]
pub fn address_of(public_key: &[u8; HYBRID_PUBLIC_KEY_LEN]) -> [u8; ADDRESS_LEN] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(ADDRESS_DOMAIN);
    hasher.update(public_key);
    *hasher.finalize().as_bytes()
}

/// First eight bytes of a digest, for `Debug`.
fn hex32(bytes: &[u8; 32]) -> String {
    bytes[..8].iter().map(|b| format!("{b:02x}")).collect()
}
