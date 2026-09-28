//! Signature suites: a closed registry, one trait, and a dispatcher.
//!
//! Every signature the chain accepts is named by a one-byte [`SuiteId`], and
//! the id travels with the signature in an [`crate::envelope`]. An algorithm
//! can then be retired or introduced without reinterpreting a single byte that
//! was written under the old one — the property ADR-007 calls crypto-agility.
//!
//! ## Why the registry is closed
//!
//! A suite that exists on some nodes and not others is a fork, so the set of
//! ids is a `#[repr(u8)]` enum and an unknown byte is a decode error, never a
//! fallback. Governance chooses *among* compiled suites ([`crate::agility`]);
//! adding one is a code change with its own KAT vectors.
//!
//! ## Sizes
//!
//! Every length below is asserted at compile time against the backing crate's
//! own constants, and the ACVP tests in `tests/acvp_tests.rs` check the
//! encodings against NIST's. They are final FIPS 204/205 sizes: ML-DSA-65 is
//! 3,309 bytes, not the 3,293 of the 2023 draft.

mod ed25519;
mod hybrid;
mod ml_dsa;
mod slh_dsa;

pub use ed25519::Ed25519;
pub use hybrid::{HybridMlDsa65SlhDsa128s, HybridSigningKey};
pub use ml_dsa::{MlDsa65, MlDsa87};
pub use slh_dsa::{SlhDsaSha2_128s, SlhDsaShake256f};

use secrecy::{ExposeSecret as _, ExposeSecretMut as _, SecretBox};
use zeroize::{ZeroizeOnDrop, Zeroizing};

/// Length of the master seed every suite derives its key from.
pub const SEED_LEN: usize = 32;

/// A one-byte algorithm identifier, carried by every envelope.
///
/// The numbering groups families by high nibble: `0x0_` classical, `0x1_`
/// lattice, `0x2_` hash-based, `0x3_` hybrids. Values are wire format and are
/// never reassigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum SuiteId {
    /// Ed25519 (RFC 8032). Legacy and devnet only: zero post-quantum security.
    Ed25519 = 0x01,
    /// ML-DSA-65 (FIPS 204), NIST category 3.
    MlDsa65 = 0x10,
    /// ML-DSA-87 (FIPS 204), NIST category 5. The mainnet default.
    MlDsa87 = 0x11,
    /// SLH-DSA-SHA2-128s (FIPS 205), NIST category 1.
    SlhDsaSha2_128s = 0x20,
    /// SLH-DSA-SHAKE-256f (FIPS 205), NIST category 5. For cold vaults.
    SlhDsaShake256f = 0x21,
    /// ML-DSA-65 and SLH-DSA-SHA2-128s, both of which must verify. Byte for
    /// byte the pair `custom_l1_node::crypto::hybrid` has always written.
    HybridMlDsa65SlhDsa128s = 0x30,
}

impl SuiteId {
    /// Every suite, in id order.
    pub const ALL: [Self; 6] = [
        Self::Ed25519,
        Self::MlDsa65,
        Self::MlDsa87,
        Self::SlhDsaSha2_128s,
        Self::SlhDsaShake256f,
        Self::HybridMlDsa65SlhDsa128s,
    ];

    /// The wire byte.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self as u8
    }

    /// This suite's registry row.
    #[must_use]
    pub const fn info(self) -> &'static SuiteInfo {
        match self {
            Self::Ed25519 => &REGISTRY[0],
            Self::MlDsa65 => &REGISTRY[1],
            Self::MlDsa87 => &REGISTRY[2],
            Self::SlhDsaSha2_128s => &REGISTRY[3],
            Self::SlhDsaShake256f => &REGISTRY[4],
            Self::HybridMlDsa65SlhDsa128s => &REGISTRY[5],
        }
    }
}

impl TryFrom<u8> for SuiteId {
    type Error = SuiteError;

    fn try_from(byte: u8) -> Result<Self, SuiteError> {
        Self::ALL
            .into_iter()
            .find(|id| id.to_byte() == byte)
            .ok_or(SuiteError::UnknownSuite(byte))
    }
}

/// One row of the registry: everything the policy and the audit need to know
/// about a suite without instantiating it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuiteInfo {
    /// The id this row describes.
    pub id: SuiteId,
    /// Human-readable name, as the standard spells it.
    pub name: &'static str,
    /// The standard, with its status.
    pub standard: &'static str,
    /// Exact encoded public key length.
    pub public_key_len: usize,
    /// Exact encoded signature length.
    pub signature_len: usize,
    /// NIST security category (1–5); 0 for none.
    pub nist_category: u8,
    /// Claimed security against a quantum adversary, in bits. A hybrid is
    /// arguably as strong as its *stronger* half, since a forger must break
    /// both; the weaker half is what is recorded, so the audit errs safe.
    pub pq_security_bits: u16,
    /// Whether the suite may sign on mainnet at all.
    pub mainnet_allowed: bool,
}

/// The registry. Row order matches [`SuiteId::info`].
pub const REGISTRY: [SuiteInfo; 6] = [
    SuiteInfo {
        id: SuiteId::Ed25519,
        name: "Ed25519",
        standard: "RFC 8032 (classical)",
        public_key_len: ed25519::PUBLIC_KEY_LEN,
        signature_len: ed25519::SIGNATURE_LEN,
        nist_category: 0,
        pq_security_bits: 0,
        mainnet_allowed: false,
    },
    SuiteInfo {
        id: SuiteId::MlDsa65,
        name: "ML-DSA-65",
        standard: "FIPS 204 (final)",
        public_key_len: ml_dsa::ML_DSA_65_PUBLIC_KEY_LEN,
        signature_len: ml_dsa::ML_DSA_65_SIGNATURE_LEN,
        nist_category: 3,
        pq_security_bits: 192,
        mainnet_allowed: true,
    },
    SuiteInfo {
        id: SuiteId::MlDsa87,
        name: "ML-DSA-87",
        standard: "FIPS 204 (final)",
        public_key_len: ml_dsa::ML_DSA_87_PUBLIC_KEY_LEN,
        signature_len: ml_dsa::ML_DSA_87_SIGNATURE_LEN,
        nist_category: 5,
        pq_security_bits: 256,
        mainnet_allowed: true,
    },
    SuiteInfo {
        id: SuiteId::SlhDsaSha2_128s,
        name: "SLH-DSA-SHA2-128s",
        standard: "FIPS 205 (final)",
        public_key_len: slh_dsa::SHA2_128S_PUBLIC_KEY_LEN,
        signature_len: slh_dsa::SHA2_128S_SIGNATURE_LEN,
        nist_category: 1,
        pq_security_bits: 128,
        mainnet_allowed: true,
    },
    SuiteInfo {
        id: SuiteId::SlhDsaShake256f,
        name: "SLH-DSA-SHAKE-256f",
        standard: "FIPS 205 (final)",
        public_key_len: slh_dsa::SHAKE_256F_PUBLIC_KEY_LEN,
        signature_len: slh_dsa::SHAKE_256F_SIGNATURE_LEN,
        nist_category: 5,
        pq_security_bits: 256,
        mainnet_allowed: true,
    },
    SuiteInfo {
        id: SuiteId::HybridMlDsa65SlhDsa128s,
        name: "ML-DSA-65+SLH-DSA-SHA2-128s",
        standard: "FIPS 204 + FIPS 205, both must verify",
        public_key_len: hybrid::PUBLIC_KEY_LEN,
        signature_len: hybrid::SIGNATURE_LEN,
        nist_category: 1,
        pq_security_bits: 128,
        mainnet_allowed: true,
    },
];

/// Errors from suite operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SuiteError {
    /// The byte names no suite in the registry.
    #[error("unknown signature suite 0x{0:02x}")]
    UnknownSuite(u8),
    /// A public key had the wrong length for its suite.
    #[error("{suite:?} public key must be {expected} bytes, got {got}")]
    PublicKeyLength {
        suite: SuiteId,
        expected: usize,
        got: usize,
    },
    /// A signature had the wrong length for its suite.
    #[error("{suite:?} signature must be {expected} bytes, got {got}")]
    SignatureLength {
        suite: SuiteId,
        expected: usize,
        got: usize,
    },
    /// The public key bytes are not a valid encoding.
    #[error("{0:?} public key is malformed")]
    MalformedPublicKey(SuiteId),
    /// The signature does not verify. Deliberately carries no detail about
    /// *why*: which check failed is an oracle to a forger.
    #[error("{0:?} signature verification failed")]
    Verification(SuiteId),
    /// The backend refused to sign (ML-DSA's context-length check).
    #[error("{0:?} signing failed")]
    Signing(SuiteId),
    /// The operating system entropy source failed during key generation.
    #[error("OS entropy source unavailable")]
    Entropy,
    /// A key file could not be decoded (PKCS#8).
    #[error("{0:?} key encoding is invalid")]
    KeyEncoding(SuiteId),
}

/// The master seed a suite's signing key is derived from.
///
/// Every suite takes the same 32 bytes and expands them under its own domain
/// string, so one wallet seed can hold a key in each suite without any two
/// sharing material. Held in a `secrecy::SecretBox`: zeroized on drop, never
/// `Clone`, and every read is a visible `expose_secret()`.
pub struct MasterSeed(SecretBox<[u8; SEED_LEN]>);

impl MasterSeed {
    /// Wraps existing seed bytes. The caller's copy is the caller's to wipe.
    #[must_use]
    pub fn from_bytes(bytes: [u8; SEED_LEN]) -> Self {
        Self(SecretBox::new(Box::new(bytes)))
    }

    /// Draws a fresh seed from the operating system.
    ///
    /// # Errors
    ///
    /// [`SuiteError::Entropy`] if the OS generator fails. Fatal, not retried.
    pub fn generate() -> Result<Self, SuiteError> {
        let mut seed = SecretBox::new(Box::new([0u8; SEED_LEN]));
        getrandom::fill(seed.expose_secret_mut()).map_err(|_| SuiteError::Entropy)?;
        Ok(Self(seed))
    }

    /// The seed bytes.
    #[must_use]
    pub fn expose(&self) -> &[u8; SEED_LEN] {
        self.0.expose_secret()
    }
}

impl core::fmt::Debug for MasterSeed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("MasterSeed(<redacted>)")
    }
}

/// One signature algorithm.
///
/// Signing keys are `ZeroizeOnDrop` by bound, so no implementation can
/// forget. Verification takes byte slices and checks their exact length
/// itself — the envelope's lengths are attacker-chosen.
pub trait SignatureSuite {
    /// The registry id.
    const ID: SuiteId;
    /// The secret key type.
    type SigningKey: ZeroizeOnDrop;

    /// Derives the signing key from a master seed, under this suite's domain.
    fn signing_key_from_seed(seed: &MasterSeed) -> Self::SigningKey;

    /// The encoded public key.
    fn public_key(key: &Self::SigningKey) -> Vec<u8>;

    /// Signs `message`.
    ///
    /// # Errors
    ///
    /// [`SuiteError::Signing`] if the backend refuses.
    fn sign(key: &Self::SigningKey, message: &[u8]) -> Result<Vec<u8>, SuiteError>;

    /// Verifies `signature` over `message` under `public_key`.
    ///
    /// # Errors
    ///
    /// A length error if either input is the wrong size for this suite, and
    /// [`SuiteError::Verification`] if the signature does not verify.
    fn verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), SuiteError>;
}

/// Domain of the account address a suite-tagged key controls. Defined here,
/// beside the suites, so every consumer of an address (the node, the wallet,
/// the chat app) derives it from one implementation.
pub const SUITE_ADDRESS_DOMAIN: &str = "maya2c 2026-09-21 suite-tagged account address v1";

/// The account address of a suite-tagged public key:
/// `BLAKE3-derive-key(SUITE_ADDRESS_DOMAIN, suite byte ‖ key)`. The suite byte
/// is inside the hash, so the same bytes read as a key of another suite name a
/// different account. Consensus: the node's `suite_address` is this function.
#[must_use]
pub fn suite_address(suite: SuiteId, public_key: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(SUITE_ADDRESS_DOMAIN);
    hasher.update(&[suite.to_byte()]);
    hasher.update(public_key);
    *hasher.finalize().as_bytes()
}

/// Verifies under whichever suite `id` names.
///
/// # Errors
///
/// As [`SignatureSuite::verify`].
pub fn verify(
    id: SuiteId,
    public_key: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<(), SuiteError> {
    match id {
        SuiteId::Ed25519 => Ed25519::verify(public_key, message, signature),
        SuiteId::MlDsa65 => MlDsa65::verify(public_key, message, signature),
        SuiteId::MlDsa87 => MlDsa87::verify(public_key, message, signature),
        SuiteId::SlhDsaSha2_128s => SlhDsaSha2_128s::verify(public_key, message, signature),
        SuiteId::SlhDsaShake256f => SlhDsaShake256f::verify(public_key, message, signature),
        SuiteId::HybridMlDsa65SlhDsa128s => {
            HybridMlDsa65SlhDsa128s::verify(public_key, message, signature)
        }
    }
}

/// Verifies with a FIPS 204/205 context string.
///
/// Transactions always use the empty context (see `ml_dsa.rs`); this exists
/// for callers that separate domains the FIPS way, and so the NIST ACVP
/// signature-verification vectors — which use random contexts — run through
/// the same code path the suites do. Ed25519 and the hybrid have no context
/// parameter and accept only an empty one.
///
/// # Errors
///
/// As [`SignatureSuite::verify`]; a non-empty context for a suite without
/// one is [`SuiteError::Verification`].
pub fn verify_with_context(
    id: SuiteId,
    public_key: &[u8],
    message: &[u8],
    context: &[u8],
    signature: &[u8],
) -> Result<(), SuiteError> {
    check_public_key_len(id, public_key)?;
    check_signature_len(id, signature)?;
    let ok = match id {
        SuiteId::MlDsa65 => {
            ml_dsa::verify_with::<::ml_dsa::MlDsa65>(public_key, message, context, signature)
        }
        SuiteId::MlDsa87 => {
            ml_dsa::verify_with::<::ml_dsa::MlDsa87>(public_key, message, context, signature)
        }
        SuiteId::SlhDsaSha2_128s => {
            slh_dsa::verify_with::<::slh_dsa::Sha2_128s>(public_key, message, context, signature)
        }
        SuiteId::SlhDsaShake256f => {
            slh_dsa::verify_with::<::slh_dsa::Shake256f>(public_key, message, context, signature)
        }
        SuiteId::Ed25519 | SuiteId::HybridMlDsa65SlhDsa128s => {
            return if context.is_empty() {
                verify(id, public_key, message, signature)
            } else {
                Err(SuiteError::Verification(id))
            };
        }
    };
    if ok {
        Ok(())
    } else {
        Err(SuiteError::Verification(id))
    }
}

/// Checks `bytes` is exactly `expected` long, as a public key of `suite`.
pub(crate) fn check_public_key_len(suite: SuiteId, bytes: &[u8]) -> Result<(), SuiteError> {
    let expected = suite.info().public_key_len;
    if bytes.len() == expected {
        Ok(())
    } else {
        Err(SuiteError::PublicKeyLength {
            suite,
            expected,
            got: bytes.len(),
        })
    }
}

/// Checks `bytes` is exactly `expected` long, as a signature of `suite`.
pub(crate) fn check_signature_len(suite: SuiteId, bytes: &[u8]) -> Result<(), SuiteError> {
    let expected = suite.info().signature_len;
    if bytes.len() == expected {
        Ok(())
    } else {
        Err(SuiteError::SignatureLength {
            suite,
            expected,
            got: bytes.len(),
        })
    }
}

/// Expands the master seed to `N` bytes under `domain`, with BLAKE3's XOF in
/// key-derivation mode so that no two domains can collide.
pub(crate) fn expand<const N: usize>(domain: &str, seed: &MasterSeed) -> Zeroizing<[u8; N]> {
    let mut out = Zeroizing::new([0u8; N]);
    let mut hasher = blake3::Hasher::new_derive_key(domain);
    hasher.update(seed.expose());
    hasher.finalize_xof().fill(out.as_mut());
    out
}

#[cfg(test)]
mod tests;
