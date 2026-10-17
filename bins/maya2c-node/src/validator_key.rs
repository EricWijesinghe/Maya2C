//! The DAG-BFT validator key file: sealed at rest, or legacy plaintext hex.
//!
//! A sealed file (`maya_signer::sealed`, Argon2id + ChaCha20-Poly1305) is
//! opened with a passphrase read from `--validator-passphrase-file` or the
//! file named by `MAYA2C_VALIDATOR_PASSPHRASE_FILE`. Never from the command
//! line or the environment value itself: both are visible to every process
//! that can list this one. A plaintext key still loads on devnets with a
//! warning, and a production build refuses it, so a mainnet validator cannot
//! run with its consensus key readable by anyone who copies the file.

use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use custom_l1_node::crypto::keys::{self, SigningKey};
use maya_signer::sealed;
use zeroize::Zeroizing;

use sealed::{PASSPHRASE_FILE_ENV, VALIDATOR_KEY_KIND as KIND};

/// The passphrase file: the flag wins over the environment variable.
fn passphrase_path(flag: Option<&Path>) -> Option<PathBuf> {
    flag.map(Path::to_path_buf)
        .or_else(|| std::env::var_os(PASSPHRASE_FILE_ENV).map(PathBuf::from))
}

fn read_passphrase(flag: Option<&Path>) -> Result<Option<Zeroizing<Vec<u8>>>, Box<dyn Error>> {
    let Some(path) = passphrase_path(flag) else {
        return Ok(None);
    };
    let pass = sealed::read_passphrase_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Some(pass))
}

/// Creates `path` for writing, refusing to overwrite, owner-only on Unix.
fn create_new(path: &Path) -> Result<std::fs::File, Box<dyn Error>> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options
        .open(path)
        .map_err(|e| format!("{}: {e} (refusing to overwrite a key)", path.display()))?)
}

/// The file contents for `key`: sealed when a passphrase is given.
fn encode(key: &SigningKey, passphrase: Option<&[u8]>) -> Result<Zeroizing<String>, Box<dyn Error>> {
    let secret = Zeroizing::new(key.to_bytes().to_vec());
    Ok(Zeroizing::new(match passphrase {
        Some(pass) => sealed::seal(&secret, &key.verifying_key().to_bytes(), KIND, pass)?,
        None => hex::encode(secret.as_slice()),
    }))
}

fn write_key(path: &Path, text: &str) -> Result<(), Box<dyn Error>> {
    let mut file = create_new(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

/// Writes a fresh ML-DSA-65 validator key to `path` and prints the public key
/// an operator puts in genesis. Sealed when a passphrase file is configured.
pub(super) fn generate(path: &Path, passphrase_flag: Option<&Path>) -> Result<(), Box<dyn Error>> {
    let pass = read_passphrase(passphrase_flag)?;
    if pass.is_none() {
        eprintln!(
            "warning: writing a PLAINTEXT validator key; set --validator-passphrase-file \
             or {PASSPHRASE_FILE_ENV} to seal it"
        );
    }
    let key = keys::generate_signing_key()?;
    write_key(path, &encode(&key, pass.as_deref().map(Vec::as_slice))?)?;
    println!("{}", hex::encode(key.verifying_key().to_bytes()));
    Ok(())
}

/// Seals the plaintext key at `input` into a new file at `output`, then opens
/// the new file again and checks it yields the same key before reporting
/// success. The plaintext file is left for the operator to destroy.
pub(super) fn seal_file(
    input: &Path,
    output: &Path,
    passphrase_flag: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let pass = read_passphrase(passphrase_flag)?.ok_or_else(|| {
        format!("sealing needs --validator-passphrase-file or {PASSPHRASE_FILE_ENV}")
    })?;
    let text = Zeroizing::new(
        std::fs::read_to_string(input).map_err(|e| format!("{}: {e}", input.display()))?,
    );
    if sealed::is_sealed(&text) {
        return Err(format!("{} is already sealed", input.display()).into());
    }
    let key = from_hex(&text, input)?;
    write_key(output, &encode(&key, Some(&pass))?)?;
    let reopened = load(output, passphrase_flag)?;
    if reopened.verifying_key().to_bytes() != key.verifying_key().to_bytes() {
        return Err("sealed file did not reopen to the same key".into());
    }
    println!("{}", hex::encode(key.verifying_key().to_bytes()));
    Ok(())
}

fn from_hex(text: &str, path: &Path) -> Result<SigningKey, Box<dyn Error>> {
    let bytes = Zeroizing::new(hex::decode(text.trim())?);
    from_bytes(&bytes, path)
}

fn from_bytes(bytes: &[u8], path: &Path) -> Result<SigningKey, Box<dyn Error>> {
    let array: Zeroizing<[u8; keys::SECRET_KEY_LEN]> = Zeroizing::new(
        bytes
            .try_into()
            .map_err(|_| format!("{}: not an ML-DSA-65 secret key", path.display()))?,
    );
    Ok(SigningKey::from_bytes(&array)?)
}

/// Reads a key written by [`generate`] or [`seal_file`].
pub(super) fn load(path: &Path, passphrase_flag: Option<&Path>) -> Result<Arc<SigningKey>, Box<dyn Error>> {
    let text = Zeroizing::new(
        std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
    );
    if !sealed::is_sealed(&text) {
        if cfg!(feature = "production") {
            return Err(format!(
                "{}: plaintext validator key refused by a production build; \
                 seal it with --seal-validator-key",
                path.display()
            )
            .into());
        }
        eprintln!(
            "warning: {} is a PLAINTEXT validator key; seal it with --seal-validator-key",
            path.display()
        );
        return Ok(Arc::new(from_hex(&text, path)?));
    }
    let pass = read_passphrase(passphrase_flag)?.ok_or_else(|| {
        format!(
            "{} is sealed: give --validator-passphrase-file or {PASSPHRASE_FILE_ENV}",
            path.display()
        )
    })?;
    let (secret, public) = sealed::open(&text, KIND, &pass)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let key = from_bytes(&secret, path)?;
    if key.verifying_key().to_bytes().as_slice() != public.as_slice() {
        return Err(format!("{}: secret and declared public key differ", path.display()).into());
    }
    Ok(Arc::new(key))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn pass_file(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("pass");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn sealed_generate_then_load() {
        let d = dir();
        let pass = pass_file(d.path(), "a long enough passphrase\n");
        let key = d.path().join("v.key");
        generate(&key, Some(&pass)).unwrap();
        let text = std::fs::read_to_string(&key).unwrap();
        assert!(sealed::is_sealed(&text));
        load(&key, Some(&pass)).unwrap();
    }

    #[test]
    fn sealed_without_passphrase_is_refused() {
        let d = dir();
        let pass = pass_file(d.path(), "a long enough passphrase");
        let key = d.path().join("v.key");
        generate(&key, Some(&pass)).unwrap();
        assert!(load(&key, None).is_err());
    }

    #[test]
    fn wrong_passphrase_is_refused() {
        let d = dir();
        let key = d.path().join("v.key");
        generate(&key, Some(&pass_file(d.path(), "a long enough passphrase"))).unwrap();
        let wrong = pass_file(d.path(), "another long passphrase");
        assert!(load(&key, Some(&wrong)).is_err());
    }

    #[test]
    fn seal_preserves_the_key_and_refuses_overwrite() {
        let d = dir();
        let plain = d.path().join("plain.key");
        generate(&plain, None).unwrap();
        // Read directly: a production build refuses to `load` plaintext.
        let text = std::fs::read_to_string(&plain).unwrap();
        let before = from_hex(&text, &plain).unwrap().verifying_key().to_bytes();
        let pass = pass_file(d.path(), "a long enough passphrase");
        let sealed_path = d.path().join("sealed.key");
        seal_file(&plain, &sealed_path, Some(&pass)).unwrap();
        let after = load(&sealed_path, Some(&pass)).unwrap().verifying_key().to_bytes();
        assert_eq!(before, after);
        assert!(seal_file(&plain, &sealed_path, Some(&pass)).is_err());
        assert!(seal_file(&sealed_path, &d.path().join("x"), Some(&pass)).is_err());
    }

    #[test]
    fn crlf_passphrase_file_matches_lf() {
        let d = dir();
        let key = d.path().join("v.key");
        generate(&key, Some(&pass_file(d.path(), "a long enough passphrase\r\n"))).unwrap();
        load(&key, Some(&pass_file(d.path(), "a long enough passphrase"))).unwrap();
    }

    #[cfg(not(feature = "production"))]
    #[test]
    fn plaintext_still_loads_on_devnets() {
        let d = dir();
        let key = d.path().join("v.key");
        generate(&key, None).unwrap();
        load(&key, None).unwrap();
    }

    #[cfg(feature = "production")]
    #[test]
    fn production_refuses_plaintext() {
        let d = dir();
        let key = d.path().join("v.key");
        generate(&key, None).unwrap();
        let err = load(&key, None).err().unwrap().to_string();
        assert!(err.contains("plaintext validator key refused"), "{err}");
    }
}
