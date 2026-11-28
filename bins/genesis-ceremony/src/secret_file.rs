//! Owner-only creation of the ceremony's secret files.
//!
//! The same pattern as `bins/l1-wallet/src/keystore.rs` (`create_owner_only`,
//! `restrict_permissions`): the system's own `whoami` and `icacls` on Windows
//! rather than a new dependency on the Windows security API.

use std::error::Error;
use std::fs;
use std::io::Write as _;
use std::path::Path;

/// Writes `bytes` to a new file at `path` readable only by the account running
/// the ceremony.
///
/// `create_new` makes the open fail on any existing path, a symlink included,
/// so a planted link is never followed and an existing secret is never
/// truncated. On Unix the mode is 0600 from creation. On Windows the file
/// would otherwise inherit the directory's ACL, and a fresh folder on a data
/// volume commonly grants every signed-in account modify rights; so the handle
/// shares nothing while the inherited DACL is replaced with a single entry for
/// this account, and only then is the secret written. Any failure removes the
/// file, so an unrestricted or half-written secret is never left behind.
pub fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    let mut file = create_owner_only(path)?;
    let result = restrict(path, &file).and_then(|()| {
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    });
    if result.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
    }
    result
}

/// Creates `path`, failing if anything exists there.
fn create_owner_only(path: &Path) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    // No other handle to the file can be opened before its DACL is replaced.
    #[cfg(windows)]
    std::os::windows::fs::OpenOptionsExt::share_mode(&mut options, 0);
    options.open(path)
}

/// Re-asserts 0600 on the handle, for a file system that ignored the mode.
#[cfg(unix)]
fn restrict(_path: &Path, file: &fs::File) -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Replaces the inherited DACL with full control for this account's SID only.
#[cfg(windows)]
fn restrict(path: &Path, _file: &fs::File) -> Result<(), Box<dyn Error>> {
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
        .ok_or("whoami did not report this account's SID")?;
    let icacls = Command::new(system32.join("icacls.exe"))
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &format!("*{sid}:F")])
        .output()?;
    if !icacls.status.success() {
        return Err(format!(
            "could not make {} owner-only: {}",
            path.display(),
            String::from_utf8_lossy(&icacls.stdout).trim()
        )
        .into());
    }
    Ok(())
}

/// No owner-only primitive is known here; refuse rather than write a secret
/// with whatever the directory grants.
#[cfg(not(any(unix, windows)))]
fn restrict(path: &Path, _file: &fs::File) -> Result<(), Box<dyn Error>> {
    Err(format!("cannot restrict {} on this platform", path.display()).into())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::write_secret;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "genesis-ceremony-secret-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_second_write_to_the_same_path_is_refused() {
        let path = scratch("twice").join("root-0.secret");
        write_secret(&path, b"first").expect("first write");
        assert!(write_secret(&path, b"second").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first");
    }

    #[cfg(unix)]
    #[test]
    fn a_secret_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let path = scratch("mode").join("root-0.secret");
        write_secret(&path, b"k").expect("write");
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode {mode:o}");
    }

    #[cfg(windows)]
    #[test]
    fn a_secret_has_one_uninherited_entry_for_its_owner() {
        let path = scratch("acl").join("root-0.secret");
        write_secret(&path, b"k").expect("write");
        let out = std::process::Command::new("icacls")
            .arg(&path)
            .output()
            .expect("run icacls");
        let acl = String::from_utf8_lossy(&out.stdout);
        // One line per entry, each `principal:(rights)`; `(I)` marks inherited.
        assert_eq!(acl.matches(":(").count(), 1, "expected one entry:\n{acl}");
        assert!(!acl.contains("(I)"), "an inherited entry survived:\n{acl}");
        let user = std::env::var("USERNAME").expect("USERNAME");
        assert!(acl.to_lowercase().contains(&user.to_lowercase()), "{acl}");
        assert_eq!(fs::read(&path).unwrap(), b"k");
    }
}
