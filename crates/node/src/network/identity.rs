//! The persisted libp2p identity.
//!
//! ## Why this is not private to the node binary
//!
//! A seed's `PeerId` is derived from this key, and cross-region bootnode
//! multiaddrs are written against that `PeerId` — they are baked into manifests
//! long before the pod that owns the key first starts. Something other than the
//! node has to be able to read the key and report the identity, or the fleet's
//! bootnode list can only be discovered by reading logs after the fact.
//!
//! So the loader lives in the library and both `node` and `peerid` call it.
//! Two copies of this logic that disagreed by a byte would produce two different
//! `PeerId`s from the same file, and the symptom would be a seed nobody can dial.
//!
//! ## Format
//!
//! Exactly 32 bytes: the raw ed25519 secret scalar. Not PKCS#8, not hex — a
//! fixed-size opaque blob is what `kubectl create secret generic --from-file`
//! round-trips without transformation, which is how these keys reach a cluster.

use std::path::{Path, PathBuf};

use libp2p::PeerId;
use libp2p::identity::Keypair;
use zeroize::Zeroizing;

use crate::error::{NodeError, Result};

/// Filename holding the persisted identity, relative to the data directory.
pub const NODE_KEY_FILE: &str = "node_key";

/// Length of the raw ed25519 seed on disk.
const SEED_LEN: usize = 32;

/// Path of the key file inside a data directory.
#[must_use]
pub fn key_path(data_dir: &Path) -> PathBuf {
    data_dir.join(NODE_KEY_FILE)
}

/// Builds an identity error carrying the offending path.
fn identity_error(path: &Path, reason: impl Into<String>) -> NodeError {
    NodeError::Identity {
        path: path.display().to_string(),
        reason: reason.into(),
    }
}

/// Loads the identity at `<data_dir>/node_key`, creating one on first run.
///
/// # Errors
///
/// Returns [`NodeError::Identity`] if the key cannot be read or written, or if
/// an existing file is not exactly 32 bytes.
pub fn load_or_create(data_dir: &Path) -> Result<Keypair> {
    let path = key_path(data_dir);

    if path.exists() {
        return load(&path);
    }

    let keypair = Keypair::generate_ed25519();
    let seed = seed_of(&keypair).map_err(|reason| identity_error(&path, reason))?;

    write_new_owner_only(&path, &seed).map_err(|e| identity_error(&path, e.to_string()))?;

    Ok(keypair)
}

/// Loads an identity from an explicit key file.
///
/// # Errors
///
/// Returns [`NodeError::Identity`] if the file cannot be read or is not a
/// 32-byte ed25519 seed.
pub fn load(path: &Path) -> Result<Keypair> {
    let bytes = std::fs::read(path).map_err(|e| identity_error(path, e.to_string()))?;

    let mut seed = <[u8; SEED_LEN]>::try_from(bytes.as_slice()).map_err(|_| {
        identity_error(
            path,
            format!(
                "must contain exactly {SEED_LEN} bytes, found {}",
                bytes.len()
            ),
        )
    })?;

    Keypair::ed25519_from_bytes(&mut seed).map_err(|e| identity_error(path, e.to_string()))
}

/// Extracts the raw 32-byte seed from a keypair.
fn seed_of(keypair: &Keypair) -> std::result::Result<Zeroizing<Vec<u8>>, String> {
    Ok(Zeroizing::new(
        keypair
            .clone()
            .try_into_ed25519()
            .map_err(|e| format!("generated key is not ed25519: {e}"))?
            .secret()
            .as_ref()
            .to_vec(),
    ))
}

/// The `PeerId` a key file yields.
///
/// # Errors
///
/// Propagates the failures of [`load`].
pub fn peer_id_at(path: &Path) -> Result<PeerId> {
    Ok(PeerId::from(load(path)?.public()))
}

/// Creates the key file owner-only from the first byte, and never overwrites.
///
/// Mode is set at `open` rather than chmod-ed afterwards, so the seed is never
/// readable by other users; `create_new` turns a racing creator into an error.
/// Windows has no Unix mode bits: owner-only ACLs there are the ADR-049 decision.
#[cfg(unix)]
fn write_new_owner_only(path: &Path, seed: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    write_or_remove(file, path, seed)
}

/// Non-Unix: no mode bits to set, so only the no-overwrite guarantee applies.
#[cfg(not(unix))]
fn write_new_owner_only(path: &Path, seed: &[u8]) -> std::io::Result<()> {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    write_or_remove(file, path, seed)
}

/// Writes the seed, and on failure removes the file so a half-written key does
/// not block every later start with "found N bytes".
fn write_or_remove(mut file: std::fs::File, path: &Path, seed: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let result = file.write_all(seed);
    if result.is_err() {
        // Closed before removal: Windows refuses to delete an open file.
        drop(file);
        // A failed removal leaves the partial file; the write error is still returned.
        let _ = std::fs::remove_file(path);
    }
    result
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_fresh_directory_gets_a_new_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let keypair = load_or_create(dir.path()).expect("create");

        let written = std::fs::read(key_path(dir.path())).expect("read back");
        assert_eq!(written.len(), SEED_LEN);
        assert_eq!(
            PeerId::from(keypair.public()),
            peer_id_at(&key_path(dir.path())).expect("peer id")
        );
    }

    #[test]
    fn the_identity_survives_a_restart() {
        // This is the whole point of persisting it: a seed that regenerated its
        // key would invalidate every bootnode address pointing at it.
        let dir = tempfile::tempdir().expect("tempdir");

        let first = load_or_create(dir.path()).expect("create");
        let second = load_or_create(dir.path()).expect("reload");

        assert_eq!(
            PeerId::from(first.public()),
            PeerId::from(second.public()),
            "restart changed the PeerId"
        );
    }

    #[test]
    fn a_truncated_key_is_rejected_rather_than_padded() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(key_path(dir.path()), [7u8; 31]).expect("write");

        let error = load_or_create(dir.path()).expect_err("must reject");
        assert!(
            error.to_string().contains("found 31"),
            "unhelpful error: {error}"
        );
    }

    #[test]
    fn a_missing_key_file_reports_its_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("absent").join(NODE_KEY_FILE);

        let error = load(&missing).expect_err("must fail");
        assert!(
            error.to_string().contains("absent"),
            "lost the path: {error}"
        );
    }

    #[test]
    fn a_key_written_by_hand_yields_a_stable_peer_id() {
        // Operators pre-generate seed identities and ship them as Secrets, so a
        // 32-byte file from any source must load to the same PeerId every time.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = key_path(dir.path());
        std::fs::write(&path, [3u8; SEED_LEN]).expect("write");

        let first = peer_id_at(&path).expect("first");
        let second = peer_id_at(&path).expect("second");
        assert_eq!(first, second);
    }

    #[test]
    fn a_new_key_file_is_never_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = key_path(dir.path());
        std::fs::write(&path, [9u8; SEED_LEN]).expect("write");

        let error = write_new_owner_only(&path, &[1u8; SEED_LEN]).expect_err("must refuse");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).expect("read"), vec![9u8; SEED_LEN]);
    }

    #[test]
    fn a_failed_seed_write_leaves_no_partial_key_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = key_path(dir.path());
        std::fs::write(&path, []).expect("create");
        // A read-only handle makes the write fail the way a full disk would.
        let read_only = std::fs::File::open(&path).expect("open");

        write_or_remove(read_only, &path, &[1u8; SEED_LEN]).expect_err("write must fail");
        assert!(!path.exists(), "a partial key file was left behind");
    }

    #[cfg(unix)]
    #[test]
    fn a_new_key_file_is_owner_only_from_creation() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = key_path(dir.path());
        write_new_owner_only(&path, &[1u8; SEED_LEN]).expect("create");

        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "key file readable beyond its owner");
    }
}
