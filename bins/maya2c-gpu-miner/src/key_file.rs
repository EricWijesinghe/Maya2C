//! Owner-only creation of the payout key file.
//!
//! The same pattern as `bins/l1-wallet/src/keystore.rs` (`create_owner_only`,
//! `restrict_permissions`): the system's own `whoami` and `icacls` on Windows
//! rather than a new dependency on the Windows security API.

use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

/// Writes `bytes` to a new file at `path` that only this account can open.
///
/// Fails with [`io::ErrorKind::AlreadyExists`] when anything is at `path`, a
/// symlink included, so a planted link is never followed and a key that may
/// already hold payouts is never overwritten by a second first run. On Unix
/// the mode is 0600 from creation; on Windows the handle shares nothing while
/// the inherited DACL is replaced with one entry for this account. Any other
/// failure removes the file rather than leave an unrestricted key behind.
pub fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut options, 0);
    let mut file = options.open(path)?;
    let result = restrict(path, &file).and_then(|()| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    if result.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
    }
    result
}

/// Re-asserts 0600 on the handle, for a file system that ignored the mode.
#[cfg(unix)]
fn restrict(_path: &Path, file: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

/// Replaces the inherited DACL with full control for this account's SID only.
#[cfg(windows)]
fn restrict(path: &Path, _file: &fs::File) -> io::Result<()> {
    use std::process::Command;
    let system32 = std::path::PathBuf::from(
        std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()),
    )
    .join("System32");
    let whoami = Command::new(system32.join("whoami.exe"))
        .args(["/user", "/fo", "csv", "/nh"])
        .output()?;
    let sid = String::from_utf8_lossy(&whoami.stdout)
        .split(',')
        .map(|f| f.trim().trim_matches('"').to_owned())
        .find(|f| f.starts_with("S-1-"))
        .ok_or_else(|| io::Error::other("whoami did not report this account's SID"))?;
    let icacls = Command::new(system32.join("icacls.exe"))
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("*{sid}:F")])
        .output()?;
    if icacls.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "could not make {} owner-only: {}",
            path.display(),
            String::from_utf8_lossy(&icacls.stdout).trim()
        )))
    }
}

/// No owner-only primitive is known here; refuse rather than expose the key.
#[cfg(not(any(unix, windows)))]
fn restrict(path: &Path, _file: &fs::File) -> io::Result<()> {
    Err(io::Error::other(format!(
        "cannot restrict {} on this platform",
        path.display()
    )))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::write_new;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("maya-miner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_second_create_on_an_existing_path_fails() {
        let path = scratch("twice").join("miner.key");
        write_new(&path, b"first").unwrap();
        let err = write_new(&path, b"second").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&path).unwrap(), b"first");
    }

    #[cfg(unix)]
    #[test]
    fn a_planted_link_is_not_followed_and_the_key_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("link");
        let target = dir.join("outside");
        std::fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("miner.key")).unwrap();
        assert!(write_new(&dir.join("miner.key"), b"k").is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        let fresh = dir.join("fresh.key");
        write_new(&fresh, b"k").unwrap();
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode {mode:o}");
    }

    #[cfg(windows)]
    #[test]
    fn a_key_has_one_uninherited_entry_for_its_owner() {
        let path = scratch("acl").join("miner.key");
        write_new(&path, b"k").unwrap();
        let out = std::process::Command::new("icacls")
            .arg(&path)
            .output()
            .expect("run icacls");
        let acl = String::from_utf8_lossy(&out.stdout);
        // One line per entry, each `principal:(rights)`; `(I)` marks inherited.
        assert_eq!(acl.matches(":(").count(), 1, "expected one entry:\n{acl}");
        assert!(!acl.contains("(I)"), "an inherited entry survived:\n{acl}");
        assert_eq!(std::fs::read(&path).unwrap(), b"k");
    }
}
