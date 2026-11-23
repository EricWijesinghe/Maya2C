//! Key and passphrase files only their owner can open (audit NEW-01).
//!
//! Unix: a new key file gets mode 0600 from its `OpenOptions`, and a
//! passphrase file group or others can read is refused (`sealed.rs`).
//! Windows: a new file inherits its folder's DACL, which on a shared or
//! careless folder lets other accounts read it. So [`restrict`] replaces the
//! DACL of a file just created with one full-control entry for the account
//! running this process, and [`check`] refuses a file whose DACL grants any
//! account but that one, SYSTEM and Administrators: the allow list of the
//! Hub's key check (`apps/hub/src-tauri/src/key_acl.rs`).
//!
//! Windows work runs the system's own `whoami`, `icacls` and `powershell` by
//! full path under `%SystemRoot%`, as `l1-wallet`'s keystore does, rather than
//! adding a Windows security API dependency to a crate the node links.

use std::path::Path;

/// SYSTEM and the local Administrators group, as SIDs and as SDDL aliases.
const TRUSTED: [&str; 4] = ["S-1-5-18", "SY", "S-1-5-32-544", "BA"];

/// The DACL part of an SDDL string: after `D:`, up to the next top-level
/// section tag (`O:`, `G:`, `S:`). `None` when there is no DACL part.
fn dacl(sddl: &str) -> Option<&str> {
    let bytes = sddl.as_bytes();
    let mut depth = 0u32;
    let mut start = None;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b':' if depth == 0 && i > 0 => match (start, bytes[i - 1]) {
                (None, b'D') => start = Some(i + 1),
                (Some(s), b'O' | b'G' | b'S') => return Some(&sddl[s..i - 1]),
                _ => {}
            },
            _ => {}
        }
    }
    start.map(|s| &sddl[s..])
}

/// The accounts other than `me`, SYSTEM and Administrators that an SDDL
/// string's DACL grants anything to. A missing or null DACL lets everyone
/// in, and is reported as `Everyone`. Deny entries and inherit-only entries
/// grant nothing on the file itself and are skipped; every other entry
/// counts whatever its rights, so an entry this parser cannot read fails
/// closed.
#[must_use]
pub fn others_granted(sddl: &str, me: &str) -> Vec<String> {
    let Some(dacl) = dacl(sddl) else {
        return vec!["Everyone (no DACL)".to_owned()];
    };
    if dacl.starts_with("NO_ACCESS_CONTROL") {
        return vec!["Everyone (null DACL)".to_owned()];
    }
    dacl.split('(')
        .skip(1)
        .map(|ace| ace.split(')').next().unwrap_or(ace))
        .filter_map(|ace| {
            let fields: Vec<&str> = ace.split(';').collect();
            let (Some(kind), Some(flags), Some(sid)) =
                (fields.first(), fields.get(1), fields.get(5))
            else {
                return Some(format!("unreadable entry ({ace})"));
            };
            let denies = matches!(*kind, "D" | "OD" | "XD");
            let inherit_only = flags.as_bytes().chunks(2).any(|f| f == b"IO");
            let trusted = *sid == me || TRUSTED.contains(sid);
            (!denies && !inherit_only && !trusted).then(|| (*sid).to_owned())
        })
        .collect()
}

/// Makes a file this process just created readable by this account only.
/// Called on the open, unshared handle's path before any secret is written.
///
/// # Errors
///
/// When the DACL cannot be replaced (Windows); never elsewhere.
pub fn restrict(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        sys::grant_owner_only(path, &sys::current_user_sid()?)
    }
    #[cfg(not(windows))]
    {
        let _ = path; // Unix: mode 0600 at creation; a umask only removes bits.
        Ok(())
    }
}

/// Refuses a file other accounts can open (Windows). Unix modes are checked on
/// the open handle by the caller.
///
/// # Errors
///
/// [`std::io::ErrorKind::PermissionDenied`] naming the other accounts, or the
/// error that stopped the DACL being read: it fails closed.
pub fn check(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        let others = others_granted(&sys::sddl(path)?, &sys::current_user_sid()?);
        if !others.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "{} is open to other accounts ({}); make it owner-only: \
                     icacls \"{}\" /inheritance:r /grant:r \"%USERNAME%\":F",
                    path.display(),
                    others.join(", "),
                    path.display()
                ),
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(windows)]
mod sys {
    use std::io::{Error, ErrorKind, Result};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    fn system32() -> PathBuf {
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()))
            .join("System32")
    }

    fn checked(what: &str, output: &Output) -> Result<String> {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if output.status.success() && !text.is_empty() {
            Ok(text)
        } else {
            Err(Error::other(format!(
                "{what} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    /// This process's account SID (`S-1-5-21-...`), from `whoami /user`.
    pub(super) fn current_user_sid() -> Result<String> {
        let out = Command::new(system32().join("whoami.exe"))
            .args(["/user", "/fo", "csv", "/nh"])
            .output()?;
        checked("whoami", &out)?
            .split(',')
            .map(|f| f.trim().trim_matches('"').to_owned())
            .find(|f| f.starts_with("S-1-"))
            .ok_or_else(|| Error::other("whoami did not report this account's SID"))
    }

    /// The file's security descriptor in SDDL. The path travels in an
    /// environment variable, never in the command text, so no file name can
    /// change the command.
    pub(super) fn sddl(path: &Path) -> Result<String> {
        let out = Command::new(system32().join(r"WindowsPowerShell\v1.0\powershell.exe"))
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "(Get-Acl -LiteralPath $env:MAYA2C_ACL_PATH).Sddl",
            ])
            .env("MAYA2C_ACL_PATH", path)
            .output()?;
        checked("reading the file's access list", &out)
    }

    /// Replaces the inherited DACL with full control for `sid` alone.
    pub(super) fn grant_owner_only(path: &Path, sid: &str) -> Result<()> {
        let out = Command::new(system32().join("icacls.exe"))
            .arg(path)
            .args(["/inheritance:r", "/grant:r", &format!("*{sid}:F")])
            .output()?;
        if out.status.success() {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::PermissionDenied,
                format!(
                    "could not make {} owner-only: {}",
                    path.display(),
                    String::from_utf8_lossy(&out.stdout).trim()
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::others_granted;

    const ME: &str = "S-1-5-21-1-2-3-1001";

    #[test]
    fn owner_system_and_administrators_only_is_private() {
        let sddl = format!("O:{ME}G:{ME}D:PAI(A;;FA;;;{ME})(A;;FA;;;SY)(A;;FA;;;BA)");
        assert!(others_granted(&sddl, ME).is_empty());
    }

    #[test]
    fn everyone_users_or_another_account_is_reported() {
        for other in ["WD", "BU", "AU", "S-1-5-21-1-2-3-1002"] {
            let sddl = format!("O:{ME}D:(A;;FA;;;{ME})(A;ID;FR;;;{other})");
            assert_eq!(others_granted(&sddl, ME), vec![other.to_owned()], "{other}");
        }
    }

    #[test]
    fn deny_and_inherit_only_entries_grant_nothing() {
        let sddl = format!("D:(D;;FA;;;WD)(A;OICIIO;FA;;;CO)(A;;FA;;;{ME})");
        assert!(others_granted(&sddl, ME).is_empty());
    }

    #[test]
    fn a_null_or_missing_dacl_is_open_to_everyone() {
        assert_eq!(others_granted("O:BAD:NO_ACCESS_CONTROL", ME).len(), 1);
        assert_eq!(others_granted(&format!("O:{ME}G:{ME}"), ME).len(), 1);
    }

    #[test]
    fn a_sacl_after_the_dacl_is_not_read_as_a_grant() {
        let sddl = format!("O:{ME}D:P(A;;FA;;;{ME})S:(ML;;NW;;;LW)");
        assert!(others_granted(&sddl, ME).is_empty());
    }

    #[test]
    fn an_entry_it_cannot_read_fails_closed() {
        assert_eq!(others_granted("D:(A;;FA)", ME).len(), 1);
    }
}
