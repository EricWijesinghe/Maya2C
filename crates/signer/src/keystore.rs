//! Encrypted local keystore — the development and "no PQ-capable HSM" backend
//! (ADR-022).
//!
//! The 32-byte master seed is sealed with ChaCha20-Poly1305 under a key
//! stretched from the operator's passphrase by Argon2id (64 MiB, 3 passes).
//! The file also carries the ML-DSA-65 public key in the clear, so a node can
//! be configured with it without the passphrase. The seed is decrypted into a
//! [`MasterSeed`] (zeroized on drop) and never written anywhere else.

use std::path::Path;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use maya_crypto_pq::suite::{MasterSeed, MlDsa65, SignatureSuite};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Argon2id memory, KiB. 64 MiB: slow enough to make guessing a weak
/// passphrase expensive, small enough for a signer box.
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_PASSES: u32 = 3;
const FORMAT: u32 = 1;

/// Keystore failures.
#[derive(Debug, thiserror::Error)]
pub enum KeystoreError {
    /// File I/O.
    #[error("keystore file: {0}")]
    Io(#[from] std::io::Error),
    /// Malformed file.
    #[error("keystore format: {0}")]
    Format(String),
    /// Wrong passphrase or a tampered file: the AEAD tag did not verify.
    #[error("keystore could not be decrypted (wrong passphrase or tampered file)")]
    Decrypt,
    /// The OS random source failed.
    #[error("no entropy for keystore salt/nonce")]
    Entropy,
}

#[derive(Serialize, Deserialize)]
struct File {
    format: u32,
    kdf: String,
    kdf_memory_kib: u32,
    kdf_passes: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
    ml_dsa_65_public_key: String,
}

fn key_from(
    passphrase: &[u8],
    salt: &[u8],
    memory: u32,
    passes: u32,
) -> Result<Zeroizing<[u8; 32]>, KeystoreError> {
    let params = Params::new(memory, passes, 1, Some(32))
        .map_err(|e| KeystoreError::Format(e.to_string()))?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase, salt, key.as_mut())
        .map_err(|e| KeystoreError::Format(e.to_string()))?;
    Ok(key)
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .fold(String::with_capacity(b.len() * 2), |mut s, x| {
            use std::fmt::Write;
            let _ = write!(s, "{x:02x}");
            s
        })
}

fn unhex(s: &str) -> Result<Vec<u8>, KeystoreError> {
    if !s.len().is_multiple_of(2) {
        return Err(KeystoreError::Format("odd hex".into()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| KeystoreError::Format("bad hex".into()))
        })
        .collect()
}

/// Writes a new keystore for `seed` at `path`, sealed under `passphrase`.
///
/// # Errors
///
/// [`KeystoreError`] on I/O or entropy failure.
pub fn create(path: &Path, seed: &MasterSeed, passphrase: &[u8]) -> Result<Vec<u8>, KeystoreError> {
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut salt).map_err(|_| KeystoreError::Entropy)?;
    getrandom::fill(&mut nonce).map_err(|_| KeystoreError::Entropy)?;
    let key = key_from(passphrase, &salt, KDF_MEMORY_KIB, KDF_PASSES)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), seed.expose().as_slice())
        .map_err(|_| KeystoreError::Decrypt)?;
    let public = MlDsa65::public_key(&MlDsa65::signing_key_from_seed(seed));
    let file = File {
        format: FORMAT,
        kdf: "argon2id".into(),
        kdf_memory_kib: KDF_MEMORY_KIB,
        kdf_passes: KDF_PASSES,
        salt: hex(&salt),
        nonce: hex(&nonce),
        ciphertext: hex(&ciphertext),
        ml_dsa_65_public_key: hex(&public),
    };
    let text =
        serde_json::to_string_pretty(&file).map_err(|e| KeystoreError::Format(e.to_string()))?;
    std::fs::write(path, text)?;
    Ok(public)
}

/// Decrypts the keystore at `path`.
///
/// # Errors
///
/// [`KeystoreError::Decrypt`] for a wrong passphrase or a tampered file, and
/// if the decrypted seed does not produce the public key the file declares.
pub fn open(path: &Path, passphrase: &[u8]) -> Result<MasterSeed, KeystoreError> {
    let file: File = serde_json::from_str(&std::fs::read_to_string(path)?)
        .map_err(|e| KeystoreError::Format(e.to_string()))?;
    if file.format != FORMAT || file.kdf != "argon2id" {
        return Err(KeystoreError::Format(format!(
            "format {} kdf {}",
            file.format, file.kdf
        )));
    }
    let key = key_from(
        passphrase,
        &unhex(&file.salt)?,
        file.kdf_memory_kib,
        file.kdf_passes,
    )?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let nonce = unhex(&file.nonce)?;
    if nonce.len() != 12 {
        return Err(KeystoreError::Format("nonce length".into()));
    }
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                unhex(&file.ciphertext)?.as_slice(),
            )
            .map_err(|_| KeystoreError::Decrypt)?,
    );
    let mut bytes: [u8; 32] = plain
        .as_slice()
        .try_into()
        .map_err(|_| KeystoreError::Decrypt)?;
    let seed = MasterSeed::from_bytes(bytes);
    // `from_bytes` took a copy; wipe this one (Standing Order 5).
    zeroize::Zeroize::zeroize(&mut bytes);
    if hex(&MlDsa65::public_key(&MlDsa65::signing_key_from_seed(&seed)))
        != file.ml_dsa_65_public_key
    {
        return Err(KeystoreError::Decrypt);
    }
    Ok(seed)
}
