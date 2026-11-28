//! Owner-only creation and checked reading of the payout key file.
//!
//! The same pattern as `bins/l1-wallet/src/keystore.rs` (`create_owner_only`,
//! `restrict_permissions`): the system's own `whoami` and `icacls` on Windows
//! rather than a new dependency on the Windows security API.
//!
//! Reading is as strict as writing: the miner runs without the Hub's folder
//! checks, so a key planted by another account (a plain file or a link to a
//! seed they know) must be refused here, or shares pay an address they spend.

use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::Path;

use zeroize::Zeroizing;

/// Longest key file accepted: 64 hex characters plus a line ending and spaces.
const KEY_FILE_MAX: usize = 128;

/// Reads an existing key file, refusing a link, a non-regular file, and on Unix
/// a file another account owns or that group or others can open.
pub fn read_existing(path: &Path) -> io::Result<Zeroizing<Vec<u8>>> {
    let file = open_existing(path)?;
    // Sized up front so the buffer never reallocates and leaves key bytes behind.
    let mut bytes = Zeroizing::new(Vec::with_capacity(KEY_FILE_MAX + 1));
    file.take(KEY_FILE_MAX as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > KEY_FILE_MAX {
        return Err(refused(path, "is larger than a key"));
    }
    Ok(bytes)
}

/// Refuses a key folder another account owns or can write into: whoever can
/// write there can swap the key file. Same rule as the Hub's `folder_open_to_others`.
#[cfg(unix)]
pub fn check_folder(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    let meta = fs::metadata(dir)?;
    if meta.uid() != rustix::process::getuid().as_raw() || meta.mode() & 0o022 != 0 {
        return Err(refused(
            dir,
            "is owned by another account or writable by group or others (chmod go-w)",
        ));
    }
    Ok(())
}

// shortcut: no folder DACL check on Windows (the Hub has one, `key_acl.rs`, on
// the `windows` crate); port it here if the standalone miner ships for Windows users.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
pub fn check_folder(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn open_existing(path: &Path) -> io::Result<fs::File> {
    use rustix::fs::{Mode, OFlags};
    use std::os::unix::fs::MetadataExt;
    // NOFOLLOW: a link at the key path fails to open instead of being read
    // through. NONBLOCK: a planted FIFO cannot stall start-up.
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let fd = rustix::fs::open(path, flags, Mode::empty()).map_err(|e| {
        refused(
            path,
            &format!("could not be opened without following links: {e}"),
        )
    })?;
    let file = fs::File::from(fd);
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(refused(path, "is not a regular file"));
    }
    if meta.uid() != rustix::process::getuid().as_raw() {
        return Err(refused(path, "is owned by another account"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(refused(
            path,
            "can be opened by group or others (chmod 600)",
        ));
    }
    Ok(file)
}

#[cfg(windows)]
fn open_existing(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    // Opens a link itself rather than its target, so the attribute check sees it.
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let meta = file.metadata()?;
    if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !meta.is_file() {
        return Err(refused(path, "is a link or not a regular file"));
    }
    Ok(file)
}

/// No way to refuse links is known here; refuse rather than read through one.
#[cfg(not(any(unix, windows)))]
fn open_existing(path: &Path) -> io::Result<fs::File> {
    Err(refused(path, "cannot be checked on this platform"))
}

fn refused(path: &Path, why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("refusing key path {}: it {why}", path.display()),
    )
}

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
