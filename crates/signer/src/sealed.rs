//! Sealed secret files: any secret byte string at rest, for keys that are not
//! derived from a 32-byte seed (a node validator key sealed by older builds; ML-DSA-87 validator keys are a 32-byte seed).
//!
//! The same primitives as [`crate::keystore`]: Argon2id (64 MiB, 3 passes)
//! stretches the passphrase, and ChaCha20-Poly1305 seals the secret. Everything
//! in the clear (format, kind, KDF parameters, salt, public key) is bound in as
//! associated data. Editing any of it, for example to swap in another public
//! key or a cheaper KDF, fails the tag, exactly like a wrong passphrase does.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::keystore::{KDF_MEMORY_KIB, KDF_PASSES, KeystoreError, hex, key_from, unhex};

const FORMAT: u32 = 2;
/// The kind label of a node's ML-DSA-87 validator key (a 32-byte seed); the
/// label changed with the key type, so a file sealed by an older build fails
/// with a kind mismatch rather than opening as the wrong key. The node and the
/// wallet both open it, so the label lives here once.
pub const VALIDATOR_KEY_KIND: &str = "maya2c-ml-dsa-87-validator-seed";
/// The environment variable naming a validator passphrase *file*. The value
/// is a path, never the passphrase: an environment is readable by every
/// process that can inspect this one.
pub const PASSPHRASE_FILE_ENV: &str = "MAYA2C_VALIDATOR_PASSPHRASE_FILE";
/// Shorter passphrases are refused at sealing time: Argon2id slows guessing,
/// but it cannot rescue a guessable word.
pub const MIN_PASSPHRASE_LEN: usize = 12;
/// Accepted KDF parameters when opening. The cleartext parameters are bound
/// in the tag, so these bounds are not about forgery: they stop a crafted
/// file making the host hash for minutes (above) or the KDF refuse with an
/// internal error (below). Sealing always uses 64 MiB and 3 passes.
const MIN_KDF_MEMORY_KIB: u32 = 8 * 1024;
const MAX_KDF_MEMORY_KIB: u32 = 256 * 1024;
const MIN_KDF_PASSES: u32 = 1;
const MAX_KDF_PASSES: u32 = 10;

#[derive(Serialize, Deserialize)]
struct SealedFile {
    format: u32,
    kind: String,
    kdf: String,
    kdf_memory_kib: u32,
    kdf_passes: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
    public_key: String,
}

impl SealedFile {
    /// The associated data: every cleartext field except the nonce and
    /// ciphertext, length-prefixed so that no two field lists encode the same.
    fn aad(&self) -> Vec<u8> {
        let fields: [&[u8]; 7] = [
            &self.format.to_le_bytes(),
            self.kind.as_bytes(),
            self.kdf.as_bytes(),
            &self.kdf_memory_kib.to_le_bytes(),
            &self.kdf_passes.to_le_bytes(),
            self.salt.as_bytes(),
            self.public_key.as_bytes(),
        ];
        let mut aad = b"maya2c-sealed-secret".to_vec();
        for field in fields {
            aad.extend_from_slice(&(field.len() as u64).to_le_bytes());
            aad.extend_from_slice(field);
        }
        aad
    }
}

/// Reads a passphrase file, dropping one trailing newline (LF or CRLF) that
/// editors and `echo` add. Everything else, including spaces, is part of it.
///
/// # Errors
///
/// [`KeystoreError::Io`] when the file cannot be read.
pub fn read_passphrase_file(path: &std::path::Path) -> Result<Zeroizing<Vec<u8>>, KeystoreError> {
    let mut bytes = Zeroizing::new(std::fs::read(path)?);
    let trailing = if bytes.ends_with(b"\r\n") {
        2
    } else {
        usize::from(bytes.ends_with(b"\n"))
    };
    let keep = bytes.len() - trailing;
    bytes.truncate(keep);
    Ok(bytes)
}

/// Whether `text` is a sealed file rather than a legacy plaintext hex key.
#[must_use]
pub fn is_sealed(text: &str) -> bool {
    text.trim_start().starts_with('{')
}

/// Seals `secret` under `passphrase`, labelled `kind`, with `public` in the clear.
///
/// # Errors
///
/// - [`KeystoreError::Format`] for a passphrase shorter than [`MIN_PASSPHRASE_LEN`].
/// - [`KeystoreError::Entropy`] when the OS random source fails.
pub fn seal(
    secret: &[u8],
    public: &[u8],
    kind: &str,
    passphrase: &[u8],
) -> Result<String, KeystoreError> {
    if passphrase.len() < MIN_PASSPHRASE_LEN {
        return Err(KeystoreError::Format(format!(
            "passphrase shorter than {MIN_PASSPHRASE_LEN} bytes"
        )));
    }
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut salt).map_err(|_| KeystoreError::Entropy)?;
    getrandom::fill(&mut nonce).map_err(|_| KeystoreError::Entropy)?;
    let mut file = SealedFile {
        format: FORMAT,
        kind: kind.into(),
        kdf: "argon2id".into(),
        kdf_memory_kib: KDF_MEMORY_KIB,
        kdf_passes: KDF_PASSES,
        salt: hex(&salt),
        nonce: hex(&nonce),
        ciphertext: String::new(),
        public_key: hex(public),
    };
    let key = key_from(passphrase, &salt, KDF_MEMORY_KIB, KDF_PASSES)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let aad = file.aad();
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: secret,
                aad: &aad,
            },
        )
        .map_err(|_| KeystoreError::Decrypt)?;
    file.ciphertext = hex(&ciphertext);
    serde_json::to_string_pretty(&file).map_err(|e| KeystoreError::Format(e.to_string()))
}

/// Opens a sealed file of the expected `kind`, returning the secret and the
/// public key the file declares. The caller checks that the two belong
/// together, because only it knows the key type.
///
/// # Errors
///
/// - [`KeystoreError::Decrypt`] for a wrong passphrase or any tampering.
/// - [`KeystoreError::Format`] for a malformed file, another kind, or KDF
///   parameters outside the accepted bounds.
pub fn open(
    text: &str,
    kind: &str,
    passphrase: &[u8],
) -> Result<(Zeroizing<Vec<u8>>, Vec<u8>), KeystoreError> {
    let file: SealedFile =
        serde_json::from_str(text).map_err(|e| KeystoreError::Format(e.to_string()))?;
    if file.format != FORMAT || file.kdf != "argon2id" || file.kind != kind {
        return Err(KeystoreError::Format(format!(
            "format {} kdf {} kind {} (expected {FORMAT}, argon2id, {kind})",
            file.format, file.kdf, file.kind
        )));
    }
    if !(MIN_KDF_MEMORY_KIB..=MAX_KDF_MEMORY_KIB).contains(&file.kdf_memory_kib)
        || !(MIN_KDF_PASSES..=MAX_KDF_PASSES).contains(&file.kdf_passes)
    {
        return Err(KeystoreError::Format("KDF parameters out of bounds".into()));
    }
    let nonce = unhex(&file.nonce)?;
    if nonce.len() != 12 {
        return Err(KeystoreError::Format("nonce length".into()));
    }
    let key = key_from(
        passphrase,
        &unhex(&file.salt)?,
        file.kdf_memory_kib,
        file.kdf_passes,
    )?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_ref()));
    let ciphertext = unhex(&file.ciphertext)?;
    let aad = file.aad();
    let secret = Zeroizing::new(
        cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| KeystoreError::Decrypt)?,
    );
    Ok((secret, unhex(&file.public_key)?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const PASS: &[u8] = b"correct horse battery staple";
    const KIND: &str = "test-key";

    fn tamper(text: &str, field: &str, value: serde_json::Value) -> String {
        let mut json: serde_json::Value = serde_json::from_str(text).unwrap();
        json[field] = value;
        json.to_string()
    }

    #[test]
    fn round_trips() {
        let text = seal(b"secret bytes", b"public", KIND, PASS).unwrap();
        assert!(is_sealed(&text));
        let (secret, public) = open(&text, KIND, PASS).unwrap();
        assert_eq!(secret.as_slice(), b"secret bytes");
        assert_eq!(public, b"public");
    }

    #[test]
    fn secret_is_not_in_the_file() {
        let text = seal(b"VISIBLE-SECRET", b"pk", KIND, PASS).unwrap();
        assert!(!text.contains("VISIBLE-SECRET"));
        assert!(!text.contains(&hex(b"VISIBLE-SECRET")));
    }

    #[test]
    fn wrong_passphrase_fails() {
        let text = seal(b"s", b"p", KIND, PASS).unwrap();
        assert!(matches!(
            open(&text, KIND, b"not the passphrase!"),
            Err(KeystoreError::Decrypt)
        ));
    }

    #[test]
    fn swapped_public_key_fails() {
        let text = seal(b"s", b"p", KIND, PASS).unwrap();
        let forged = tamper(&text, "public_key", hex(b"attacker").into());
        assert!(matches!(
            open(&forged, KIND, PASS),
            Err(KeystoreError::Decrypt)
        ));
    }

    #[test]
    fn weakened_kdf_fails() {
        let text = seal(b"s", b"p", KIND, PASS).unwrap();
        let forged = tamper(&text, "kdf_passes", 1.into());
        assert!(matches!(
            open(&forged, KIND, PASS),
            Err(KeystoreError::Decrypt)
        ));
    }

    #[test]
    fn flipped_ciphertext_fails() {
        let text = seal(b"secret", b"p", KIND, PASS).unwrap();
        let json: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut ct = unhex(json["ciphertext"].as_str().unwrap()).unwrap();
        ct[0] ^= 1;
        let forged = tamper(&text, "ciphertext", hex(&ct).into());
        assert!(matches!(
            open(&forged, KIND, PASS),
            Err(KeystoreError::Decrypt)
        ));
    }

    #[test]
    fn swapped_salt_fails() {
        let text = seal(b"s", b"p", KIND, PASS).unwrap();
        let forged = tamper(&text, "salt", hex(&[7u8; 16]).into());
        assert!(matches!(
            open(&forged, KIND, PASS),
            Err(KeystoreError::Decrypt)
        ));
    }

    #[test]
    fn passphrase_file_drops_exactly_one_newline() {
        let dir = std::env::temp_dir().join(format!("maya-sealed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cases: [(&[u8], &[u8]); 4] = [
            (b"pass\n", b"pass"),
            (b"pass\r\n", b"pass"),
            (b"pass\n\n", b"pass\n"),
            (b"pass\r", b"pass\r"),
        ];
        for (i, (raw, want)) in cases.iter().enumerate() {
            let path = dir.join(i.to_string());
            std::fs::write(&path, raw).unwrap();
            assert_eq!(read_passphrase_file(&path).unwrap().as_slice(), *want);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn other_kind_is_refused() {
        let text = seal(b"s", b"p", KIND, PASS).unwrap();
        assert!(matches!(
            open(&text, "other", PASS),
            Err(KeystoreError::Format(_))
        ));
    }

    #[test]
    fn out_of_bounds_kdf_is_refused_before_hashing() {
        let text = seal(b"s", b"p", KIND, PASS).unwrap();
        for (field, value) in [
            ("kdf_memory_kib", MAX_KDF_MEMORY_KIB + 1),
            ("kdf_memory_kib", MIN_KDF_MEMORY_KIB - 1),
            ("kdf_passes", MAX_KDF_PASSES + 1),
            ("kdf_passes", 0),
        ] {
            let forged = tamper(&text, field, value.into());
            assert!(matches!(
                open(&forged, KIND, PASS),
                Err(KeystoreError::Format(_))
            ));
        }
    }

    #[test]
    fn short_passphrase_is_refused() {
        assert!(matches!(
            seal(b"s", b"p", KIND, b"short"),
            Err(KeystoreError::Format(_))
        ));
    }

    #[test]
    fn legacy_hex_is_not_sealed() {
        assert!(!is_sealed("6db2190cacb1885f"));
    }
}
