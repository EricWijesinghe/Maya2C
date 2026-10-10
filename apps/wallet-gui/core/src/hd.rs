//! BIP-39 mnemonics and SLIP-0010 hierarchical derivation.
//!
//! ## Why SLIP-0010 and not BIP-32
//!
//! BIP-32 defines derivation over secp256k1. Its public-key (non-hardened)
//! derivation relies on the group operation `K + tG`, which has no equivalent
//! for the clamped scalars ed25519 uses. Applying BIP-32 to an ed25519 key
//! produces something that looks like a valid key and is not the key any other
//! wallet would derive from the same mnemonic — a failure that only surfaces
//! when the user tries to recover elsewhere and finds an empty account.
//!
//! **SLIP-0010** is the specification that covers ed25519, and it supports
//! **hardened derivation only**. Every index in a path must be hardened; a
//! non-hardened index is not "less secure" here, it is undefined. [`DerivationPath`]
//! rejects them rather than silently hardening.
//!
//! ## Seed derivation
//!
//! BIP-39 seed generation is PBKDF2-HMAC-SHA512, 2048 iterations, salt
//! `"mnemonic" || passphrase`. The passphrase is a *distinct* secret from the
//! mnemonic: the same words with a different passphrase yield an entirely
//! different wallet, which is what makes plausible-deniability wallets possible
//! and why an empty passphrase must be an explicit choice.

use hmac::{Hmac, Mac};
use sha2::Sha512;
use zeroize::{Zeroize, Zeroizing};

// Re-exported, not merely imported: the chain key to ML-DSA seed bridge is this
// wallet's own construction, and a caller reasoning about account recovery needs
// to reach the same function this module derives with.
pub use custom_l1_node::crypto::hybrid::{HybridSigningKey, signing_key_from_seed};

use crate::error::{Result, WalletError};

/// SLIP-0010 curve seed for ed25519.
const ED25519_CURVE: &[u8] = b"ed25519 seed";

/// BIP-39 PBKDF2 iteration count, fixed by the specification.
const PBKDF2_ROUNDS: u32 = 2048;

/// Length of a BIP-39 seed.
pub const SEED_LEN: usize = 64;

/// The hardened-index bit.
pub const HARDENED: u32 = 0x8000_0000;

/// SLIP-44 coin type used for `Maya2C` accounts.
///
/// 931 is the registered Terra/other space; this chain has no assignment, so
/// the value is arbitrary but must stay fixed — changing it silently moves
/// every derived account.
pub const COIN_TYPE: u32 = 7331;

/// Number of words in a generated mnemonic.
///
/// 24 words carries 256 bits of entropy. 12 words (128 bits) is common and
/// adequate, but for a wallet whose recovery phrase is the only backup the
/// larger margin is cheap.
pub const MNEMONIC_WORDS: usize = 24;

/// An extended key: the key material plus its chain code.
///
/// Both halves are secret. The chain code is not a key, but combined with a
/// private key it derives every descendant, so it is zeroized alongside.
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct ExtendedKey {
    /// 32-byte private key material.
    pub key: [u8; 32],
    /// 32-byte chain code.
    pub chain_code: [u8; 32],
}

impl core::fmt::Debug for ExtendedKey {
    /// Redacted deliberately: an extended key in a log or a panic message is a
    /// compromised wallet.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("ExtendedKey(<redacted>)")
    }
}

/// A hardened-only derivation path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DerivationPath {
    indices: Vec<u32>,
}

impl DerivationPath {
    /// The standard account path: `m/44'/COIN_TYPE'/account'/0'/index'`.
    #[must_use]
    pub fn account(account: u32, index: u32) -> Self {
        Self {
            indices: vec![
                44 | HARDENED,
                COIN_TYPE | HARDENED,
                account | HARDENED,
                HARDENED,
                index | HARDENED,
            ],
        }
    }

    /// Parses a path such as `m/44'/7331'/0'/0'/0'`.
    ///
    /// # Errors
    ///
    /// Returns [`WalletError::InvalidPath`] for a malformed path or any
    /// non-hardened index — SLIP-0010 over ed25519 does not define those, so
    /// accepting one would mean inventing behaviour no other wallet shares.
    pub fn parse(path: &str) -> Result<Self> {
        let mut parts = path.split('/');
        if parts.next() != Some("m") {
            return Err(WalletError::InvalidPath(format!(
                "{path} must begin with 'm'"
            )));
        }

        let mut indices = Vec::new();
        for part in parts {
            let Some(number) = part.strip_suffix('\'').or_else(|| part.strip_suffix('h')) else {
                return Err(WalletError::InvalidPath(format!(
                    "{path}: index '{part}' is not hardened; ed25519 derivation \
                     is hardened-only under SLIP-0010"
                )));
            };
            let value: u32 = number.parse().map_err(|_| {
                WalletError::InvalidPath(format!("{path}: '{number}' is not an index"))
            })?;
            if value >= HARDENED {
                return Err(WalletError::InvalidPath(format!(
                    "{path}: index {value} is out of range"
                )));
            }
            indices.push(value | HARDENED);
        }

        if indices.is_empty() {
            return Err(WalletError::InvalidPath(format!("{path} has no indices")));
        }

        Ok(Self { indices })
    }

    /// The raw hardened indices.
    #[must_use]
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }
}

impl core::fmt::Display for DerivationPath {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("m")?;
        for index in &self.indices {
            write!(f, "/{}'", index & !HARDENED)?;
        }
        Ok(())
    }
}

/// Generates a fresh BIP-39 mnemonic from OS entropy.
///
/// # Errors
///
/// Returns [`WalletError::Entropy`] if the OS entropy source fails. Falling
/// back to anything weaker would produce a wallet an attacker can reproduce.
pub fn generate_mnemonic() -> Result<Zeroizing<String>> {
    let mut entropy = Zeroizing::new([0u8; 32]);
    getrandom::fill(entropy.as_mut())
        .map_err(|e| WalletError::Entropy(format!("OS entropy unavailable: {e}")))?;

    let mnemonic = bip39::Mnemonic::from_entropy(entropy.as_ref())
        .map_err(|e| WalletError::Mnemonic(e.to_string()))?;

    Ok(Zeroizing::new(mnemonic.to_string()))
}

/// Validates a mnemonic, including its checksum.
///
/// # Errors
///
/// Returns [`WalletError::Mnemonic`] if the words are not in the wordlist, the
/// count is wrong, or the checksum fails. The checksum is what catches a single
/// mistyped word — without it a typo silently yields a different, empty wallet.
pub fn validate_mnemonic(phrase: &str) -> Result<()> {
    bip39::Mnemonic::parse_normalized(phrase.trim())
        .map(|_| ())
        .map_err(|e| WalletError::Mnemonic(e.to_string()))
}

/// Number of words in a phrase.
#[must_use]
pub fn word_count(phrase: &str) -> usize {
    phrase.split_whitespace().count()
}

/// Derives the BIP-39 seed from a mnemonic and passphrase.
///
/// # Errors
///
/// Returns [`WalletError::Mnemonic`] if the phrase is invalid.
pub fn seed_from_mnemonic(phrase: &str, passphrase: &str) -> Result<Zeroizing<[u8; SEED_LEN]>> {
    validate_mnemonic(phrase)?;

    let normalized = phrase.trim();
    let mut salt = Zeroizing::new(String::with_capacity(8 + passphrase.len()));
    salt.push_str("mnemonic");
    salt.push_str(passphrase);

    let mut seed = Zeroizing::new([0u8; SEED_LEN]);
    pbkdf2::pbkdf2::<Hmac<Sha512>>(
        normalized.as_bytes(),
        salt.as_bytes(),
        PBKDF2_ROUNDS,
        seed.as_mut(),
    )
    .map_err(|e| WalletError::Mnemonic(format!("seed derivation failed: {e}")))?;

    Ok(seed)
}

/// The SLIP-0010 master key for ed25519.
///
/// # Errors
///
/// Returns [`WalletError::Derivation`] if the HMAC cannot be constructed.
pub fn master_key(seed: &[u8]) -> Result<ExtendedKey> {
    let mut mac = Hmac::<Sha512>::new_from_slice(ED25519_CURVE)
        .map_err(|e| WalletError::Derivation(e.to_string()))?;
    mac.update(seed);
    let digest = mac.finalize().into_bytes();

    let mut key = [0u8; 32];
    let mut chain_code = [0u8; 32];
    key.copy_from_slice(&digest[..32]);
    chain_code.copy_from_slice(&digest[32..]);

    Ok(ExtendedKey { key, chain_code })
}

/// Derives one hardened child.
///
/// The SLIP-0010 ed25519 step is
/// `HMAC-SHA512(chain_code, 0x00 || key || index_be)`.
///
/// # Errors
///
/// Returns [`WalletError::Derivation`] if `index` is not hardened, or the HMAC
/// fails.
pub fn derive_child(parent: &ExtendedKey, index: u32) -> Result<ExtendedKey> {
    if index < HARDENED {
        return Err(WalletError::Derivation(format!(
            "index {index} is not hardened; SLIP-0010 over ed25519 defines \
             hardened derivation only"
        )));
    }

    let mut mac = Hmac::<Sha512>::new_from_slice(&parent.chain_code)
        .map_err(|e| WalletError::Derivation(e.to_string()))?;
    // The leading zero byte is what distinguishes the ed25519 construction
    // from BIP-32's, which prepends a compressed public key instead.
    mac.update(&[0u8]);
    mac.update(&parent.key);
    mac.update(&index.to_be_bytes());
    let digest = mac.finalize().into_bytes();

    let mut key = [0u8; 32];
    let mut chain_code = [0u8; 32];
    key.copy_from_slice(&digest[..32]);
    chain_code.copy_from_slice(&digest[32..]);

    Ok(ExtendedKey { key, chain_code })
}

/// Derives the extended key at `path`.
///
/// # Errors
///
/// Propagates derivation failures.
pub fn derive_path(seed: &[u8], path: &DerivationPath) -> Result<ExtendedKey> {
    let mut current = master_key(seed)?;
    for index in path.indices() {
        current = derive_child(&current, *index)?;
    }
    Ok(current)
}

/// Derives the signing key at `path`.
///
/// ## SLIP-0010 chain key to two scheme seeds
///
/// SLIP-0010 produces a 32-byte key per hardened step. An account needs seeds
/// for two signature schemes now, so the chain key at `path` is split into two
/// domain-separated seeds — one for ML-DSA-65, one for SLH-DSA-SHA2-128s — by
/// [`custom_l1_node::crypto::hybrid::signing_key_from_seed`]. One mnemonic
/// still yields one reproducible set of accounts, and the two halves of each
/// account remain independent of one another.
///
/// **This construction is specific to `Maya2C`.** SLIP-0010 defines curves for
/// ed25519 and secp256k1 and says nothing about post-quantum schemes; no
/// standard covers hierarchical derivation for either ML-DSA or SLH-DSA, and no
/// other wallet implements this mapping. A user who takes these words to a
/// different wallet will not find these accounts. The derivation path is
/// unchanged and the mnemonic is still a BIP-39 mnemonic, which makes the
/// divergence easy to miss — so it is stated here, in
/// [`custom_l1_node::crypto::hybrid::signing_key_from_seed`], and it belongs in
/// the wallet's recovery documentation too.
///
/// Note also that these are not the same accounts an earlier build derived from
/// the same mnemonic. The seed split and the two-key address are both new, so
/// the same words now name different addresses.
///
/// # Errors
///
/// Propagates derivation failures, and reports a key generation failure.
pub fn signing_key_at(seed: &[u8], path: &DerivationPath) -> Result<HybridSigningKey> {
    let extended = derive_path(seed, path)?;
    signing_key_from_seed(&extended.key)
        .map_err(|e| WalletError::Derivation(format!("{path}: {e}")))
}

/// Derives the account address at `path`.
///
/// # Errors
///
/// Propagates derivation failures.
pub fn address_at(seed: &[u8], path: &DerivationPath) -> Result<[u8; 32]> {
    Ok(signing_key_at(seed, path)?.address())
}
