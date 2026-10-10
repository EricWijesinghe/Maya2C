//! Encrypted seed storage in the OS keychain.
//!
//! ## Two layers, deliberately
//!
//! The seed is encrypted with a user passphrase *before* it reaches the
//! keychain. The keychain protects it from other users and from casual
//! extraction; the passphrase protects it from anything that can read the
//! keychain as this user — which on Windows is any process running as them.
//!
//! Either layer alone is weaker than most people assume. Together, an attacker
//! with the keychain blob still faces Argon2id at 64 MiB per guess.
//!
//! ## Backend selection
//!
//! `keyring` picks the store at compile time from the target:
//!
//! | Target | Store |
//! |---|---|
//! | Windows | Credential Manager |
//! | macOS / iOS | Apple Keychain |
//! | Android | Android Keystore |
//! | Linux | secret-service or kernel keyutils |
//!
//! Only the Windows path is exercised by this crate's tests, because that is
//! the platform they run on.
//!
//! ## Envelope format
//!
//! ```text
//! magic   8 bytes   "MAYAVLT1"
//! version 1 byte    currently 1
//! salt   16 bytes   Argon2id salt
//! nonce  12 bytes   ChaCha20-Poly1305 nonce
//! cipher N bytes    seed + 16-byte tag
//! ```

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use zeroize::Zeroizing;

use crate::error::{Result, WalletError};
use crate::hd::SEED_LEN;

const MAGIC: &[u8; 8] = b"MAYAVLT1";
const VERSION: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = 8 + 1 + SALT_LEN + NONCE_LEN;
const ENVELOPE_LEN: usize = HEADER_LEN + SEED_LEN + TAG_LEN;

/// Argon2id memory cost, in KiB (64 MiB).
///
/// This runs once per unlock with a human waiting, so the budget is far larger
/// than for a hash that runs continuously.
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_ITERATIONS: u32 = 3;
const KDF_LANES: u32 = 1;

/// Keychain service name.
const SERVICE: &str = "com.maya2c.wallet";

/// Derives the envelope key from a passphrase.
fn derive_key(passphrase: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(KDF_MEMORY_KIB, KDF_ITERATIONS, KDF_LANES, Some(32))
        .map_err(|e| WalletError::Format(format!("invalid Argon2 parameters: {e}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let mut key = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key[..])
        .map_err(|e| WalletError::Format(format!("key derivation failed: {e}")))?;
    Ok(key)
}

fn build_cipher(key: &[u8]) -> Result<ChaCha20Poly1305> {
    ChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| WalletError::Format("derived key has the wrong length".to_string()))
}

/// Encrypts a seed into a storable envelope.
///
/// # Errors
///
/// Returns [`WalletError::EmptyPassphrase`] for an empty passphrase, or
/// [`WalletError::Entropy`] if the OS entropy source fails.
pub fn seal(seed: &[u8; SEED_LEN], passphrase: &str) -> Result<Vec<u8>> {
    if passphrase.is_empty() {
        return Err(WalletError::EmptyPassphrase);
    }

    // A predictable nonce under ChaCha20 leaks the keystream, and a predictable
    // salt defeats the KDF. Both must abort rather than fall back.
    let mut salt = [0u8; SALT_LEN];
    getrandom::fill(&mut salt)
        .map_err(|e| WalletError::Entropy(format!("OS entropy unavailable: {e}")))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce_bytes)
        .map_err(|e| WalletError::Entropy(format!("OS entropy unavailable: {e}")))?;

    let key = derive_key(passphrase, &salt)?;
    let cipher = build_cipher(&key[..])?;
    let ciphertext = cipher
        .encrypt(&Nonce::from(nonce_bytes), &seed[..])
        .map_err(|_| WalletError::Format("encryption failed".to_string()))?;

    let mut out = Vec::with_capacity(ENVELOPE_LEN);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Decrypts an envelope.
///
/// # Errors
///
/// Returns [`WalletError::Format`] for a malformed envelope, or
/// [`WalletError::Unlock`] for a wrong passphrase or tampered ciphertext —
/// the two are not distinguished on purpose.
pub fn unseal(envelope: &[u8], passphrase: &str) -> Result<Zeroizing<[u8; SEED_LEN]>> {
    if envelope.len() != ENVELOPE_LEN {
        return Err(WalletError::Format(format!(
            "expected {ENVELOPE_LEN} bytes, got {}",
            envelope.len()
        )));
    }
    if &envelope[..8] != MAGIC {
        return Err(WalletError::Format("not a wallet envelope".to_string()));
    }
    let version = envelope[8];
    if version != VERSION {
        return Err(WalletError::Format(format!(
            "unsupported envelope version {version}"
        )));
    }

    let salt = &envelope[9..9 + SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    nonce_bytes.copy_from_slice(&envelope[9 + SALT_LEN..HEADER_LEN]);

    let key = derive_key(passphrase, salt)?;
    let cipher = build_cipher(&key[..])?;

    let plaintext = Zeroizing::new(
        cipher
            .decrypt(&Nonce::from(nonce_bytes), &envelope[HEADER_LEN..])
            .map_err(|_| WalletError::Unlock)?,
    );

    let mut seed = Zeroizing::new([0u8; SEED_LEN]);
    if plaintext.len() != SEED_LEN {
        return Err(WalletError::Format(
            "decrypted seed has the wrong length".into(),
        ));
    }
    seed.copy_from_slice(&plaintext);
    Ok(seed)
}

/// The OS keychain, or an in-process substitute for tests.
///
/// A trait so the storage logic can be tested without touching the real
/// Credential Manager — a test that writes to the user's actual keychain and
/// leaves entries behind is a bad neighbour.
pub trait SecretStore: Send + Sync {
    /// Stores a secret under `name`.
    ///
    /// # Errors
    ///
    /// Returns [`WalletError::Keychain`] if the store refuses.
    fn put(&self, name: &str, secret: &[u8]) -> Result<()>;

    /// Reads a secret, or `None` if absent.
    ///
    /// # Errors
    ///
    /// Returns [`WalletError::Keychain`] on a backend failure.
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>>;

    /// Deletes a secret. Absent entries are not an error.
    ///
    /// # Errors
    ///
    /// Returns [`WalletError::Keychain`] on a backend failure.
    fn delete(&self, name: &str) -> Result<()>;
}

/// Names a keychain namespace other than [`SERVICE`], so an end-to-end test
/// can drive the real application without touching the user's own wallet
/// entry (`apps/wallet-gui/e2e`). Unset in normal use.
pub const SERVICE_ENV: &str = "MAYA_WALLET_KEYCHAIN_SERVICE";

/// The real OS keychain.
#[derive(Debug, Clone)]
pub struct OsKeychain {
    service: String,
}

impl Default for OsKeychain {
    fn default() -> Self {
        Self::new()
    }
}

impl OsKeychain {
    /// A handle to the OS keychain, under [`SERVICE`] unless [`SERVICE_ENV`]
    /// names another namespace.
    #[must_use]
    pub fn new() -> Self {
        let service = std::env::var(SERVICE_ENV)
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| SERVICE.to_owned());
        Self { service }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, name)
            .map_err(|e| WalletError::Keychain(format!("opening entry '{name}': {e}")))
    }
}

impl SecretStore for OsKeychain {
    fn put(&self, name: &str, secret: &[u8]) -> Result<()> {
        self.entry(name)?
            .set_secret(secret)
            .map_err(|e| WalletError::Keychain(format!("writing '{name}': {e}")))
    }

    fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        match self.entry(name)?.get_secret() {
            Ok(secret) => Ok(Some(secret)),
            // An absent entry is a normal state, not a failure.
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(WalletError::Keychain(format!("reading '{name}': {e}"))),
        }
    }

    fn delete(&self, name: &str) -> Result<()> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(WalletError::Keychain(format!("deleting '{name}': {e}"))),
        }
    }
}

/// An in-memory [`SecretStore`] for tests.
#[derive(Debug, Default)]
pub struct MemoryStore {
    entries: std::sync::RwLock<std::collections::BTreeMap<String, Vec<u8>>>,
}

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemoryStore {
    fn put(&self, name: &str, secret: &[u8]) -> Result<()> {
        self.entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_string(), secret.to_vec());
        Ok(())
    }

    fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned())
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(name);
        Ok(())
    }
}

/// A sealed seed in a [`SecretStore`].
pub struct Vault<S: SecretStore> {
    store: S,
}

impl<S: SecretStore> Vault<S> {
    /// Wraps a store.
    #[must_use]
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Whether a wallet exists under `name`.
    ///
    /// # Errors
    ///
    /// Propagates backend failures.
    pub fn exists(&self, name: &str) -> Result<bool> {
        Ok(self.store.get(name)?.is_some())
    }

    /// Seals and stores a seed.
    ///
    /// # Errors
    ///
    /// Returns [`WalletError::WalletExists`] rather than overwriting: the
    /// stored seed may be the only copy of a key.
    pub fn store(&self, name: &str, seed: &[u8; SEED_LEN], passphrase: &str) -> Result<()> {
        if self.exists(name)? {
            return Err(WalletError::WalletExists(name.to_string()));
        }
        self.store.put(name, &seal(seed, passphrase)?)
    }

    /// Replaces an existing wallet.
    ///
    /// Separate from [`Vault::store`] so destroying a stored seed is always an
    /// explicit call, never an accident.
    ///
    /// # Errors
    ///
    /// Propagates sealing and backend failures.
    pub fn replace(&self, name: &str, seed: &[u8; SEED_LEN], passphrase: &str) -> Result<()> {
        self.store.put(name, &seal(seed, passphrase)?)
    }

    /// Unlocks a stored seed.
    ///
    /// # Errors
    ///
    /// Returns [`WalletError::NoWallet`] if absent, or [`WalletError::Unlock`]
    /// for a wrong passphrase.
    pub fn unlock(&self, name: &str, passphrase: &str) -> Result<Zeroizing<[u8; SEED_LEN]>> {
        let envelope = self
            .store
            .get(name)?
            .ok_or_else(|| WalletError::NoWallet(name.to_string()))?;
        unseal(&envelope, passphrase)
    }

    /// Deletes a stored wallet.
    ///
    /// # Errors
    ///
    /// Propagates backend failures.
    pub fn forget(&self, name: &str) -> Result<()> {
        self.store.delete(name)
    }
}
