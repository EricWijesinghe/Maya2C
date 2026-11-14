//! Key-file permissions. Windows: only the operator's account and SYSTEM may
//! touch a key (the memory-recorded rule, and `apps/hub`'s key check). Unix:
//! mode 0600. The commands are built as argument lists and run without a
//! shell, so a path with spaces or `$` is one argument.

use std::path::Path;
use std::process::Command;

use crate::devnet::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Unix,
}

impl Os {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Unix
        }
    }
}

/// One permission command: a program and its arguments.
#[derive(Debug, PartialEq, Eq)]
pub struct Step {
    pub program: &'static str,
    pub args: Vec<String>,
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

/// The commands that leave `path` readable and writable by `user` and SYSTEM
/// only. Grants come before inheritance is removed: removing inheritance
/// first would strip the operator's own access and lock the key away.
pub fn key_acl_steps(os: Os, path: &str, user: &str) -> Vec<Step> {
    match os {
        Os::Windows => vec![
            Step {
                program: "icacls",
                args: [
                    vec![path.to_owned(), "/grant:r".to_owned(), format!("{user}:F")],
                    strings(&["/grant:r", "SYSTEM:F"]),
                ]
                .concat(),
            },
            Step {
                program: "icacls",
                args: [vec![path.to_owned()], strings(&["/inheritance:r"])].concat(),
            },
        ],
        Os::Unix => vec![Step {
            program: "chmod",
            args: vec!["600".to_owned(), path.to_owned()],
        }],
    }
}

/// The account names an `icacls <file>` listing grants access to. Each line
/// is `<path> <principal>:(<rights>)`, with continuation lines indented.
pub fn principals(icacls_output: &str, path: &str) -> Vec<String> {
    icacls_output
        .lines()
        .filter_map(|line| {
            let body = line.strip_prefix(path).unwrap_or(line);
            let (who, _) = body.split_once(":(")?;
            let who = who.trim();
            (!who.is_empty()).then(|| who.to_owned())
        })
        .collect()
}

/// Whether a principal may touch a key: the operator's account or SYSTEM.
/// `DOMAIN\name` and `COMPUTER\name` compare by their last part.
pub fn is_allowed(principal: &str, user: &str) -> bool {
    let name = principal
        .rsplit('\\')
        .next()
        .unwrap_or(principal)
        .to_ascii_lowercase();
    name == user.to_ascii_lowercase() || name == "system"
}

/// The principals on `icacls_output` that are not allowed.
pub fn violations(icacls_output: &str, path: &str, user: &str) -> Vec<String> {
    principals(icacls_output, path)
        .into_iter()
        .filter(|p| !is_allowed(p, user))
        .collect()
}

/// Unix: a key readable by group or others. `None` when the mode is 0600 or
/// tighter.
pub fn unix_mode_violation(mode: u32) -> Option<String> {
    (mode & 0o077 != 0).then(|| format!("mode {:o} (group or other can access)", mode & 0o777))
}

/// The permission bits of `path`.
#[cfg(unix)]
fn unix_mode(path: &Path, shown: &str) -> Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode())
        .map_err(|e| format!("{shown}: {e}"))
}

#[cfg(not(unix))]
fn unix_mode(_path: &Path, _shown: &str) -> Result<u32> {
    Err("unix mode bits are only readable on a unix host".to_owned())
}

/// Runs one step. A nonzero exit is an error naming the command.
///
/// # Errors
///
/// The program cannot start or exits nonzero.
pub fn run_step(step: &Step) -> Result<()> {
    let status = Command::new(step.program)
        .args(&step.args)
        .status()
        .map_err(|e| format!("{}: {e}", step.program))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("{} {}: {status}", step.program, step.args.join(" ")))
}

/// The principals on `path` that the key rule does not allow, now.
///
/// # Errors
///
/// `icacls` cannot run or the mode cannot be read.
pub fn current_violations(os: Os, path: &Path, user: &str) -> Result<Vec<String>> {
    let shown = path.display().to_string();
    match os {
        Os::Windows => {
            let out = Command::new("icacls")
                .arg(path)
                .output()
                .map_err(|e| format!("icacls: {e}"))?;
            let text = String::from_utf8_lossy(&out.stdout);
            Ok(violations(&text, &shown, user))
        }
        Os::Unix => Ok(unix_mode_violation(unix_mode(path, &shown)?)
            .into_iter()
            .collect()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const PATH: &str = r"D:\Maya2C-node\validator.key";

    #[test]
    fn windows_grants_the_user_and_system_before_removing_inheritance() {
        let steps = key_acl_steps(Os::Windows, PATH, "EricW");
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0].args,
            [PATH, "/grant:r", "EricW:F", "/grant:r", "SYSTEM:F"]
        );
        assert_eq!(steps[1].args, [PATH, "/inheritance:r"]);
        assert!(steps.iter().all(|s| s.program == "icacls"));
    }

    #[test]
    fn unix_sets_mode_0600_on_the_key() {
        let steps = key_acl_steps(Os::Unix, "/home/op/validator.key", "op");
        assert_eq!(
            steps,
            [Step {
                program: "chmod",
                args: vec!["600".into(), "/home/op/validator.key".into()],
            }]
        );
    }

    const LISTING: &str = "D:\\Maya2C-node\\validator.key NT AUTHORITY\\SYSTEM:(F)\n                                        PC\\EricW:(R,W)\n                                        BUILTIN\\Administrators:(F)\n\nSuccessfully processed 1 files; Failed processing 0 files\n";

    #[test]
    fn principals_are_read_from_the_listing() {
        assert_eq!(
            principals(LISTING, PATH),
            [
                "NT AUTHORITY\\SYSTEM",
                "PC\\EricW",
                "BUILTIN\\Administrators"
            ]
        );
    }

    #[test]
    fn the_operator_and_system_are_allowed_and_administrators_is_not() {
        assert!(is_allowed("NT AUTHORITY\\SYSTEM", "EricW"));
        assert!(is_allowed("PC\\EricW", "EricW"));
        assert!(is_allowed("pc\\ericw", "EricW"));
        assert!(!is_allowed("BUILTIN\\Administrators", "EricW"));
        assert!(!is_allowed("PC\\Guest", "EricW"));
    }

    #[test]
    fn a_listing_with_an_extra_grant_reports_it() {
        assert_eq!(
            violations(LISTING, PATH, "EricW"),
            ["BUILTIN\\Administrators"]
        );
    }

    #[test]
    fn a_clean_listing_reports_nothing() {
        let clean = "D:\\Maya2C-node\\validator.key NT AUTHORITY\\SYSTEM:(F)\n                                        PC\\EricW:(F)\n";
        assert!(violations(clean, PATH, "EricW").is_empty());
    }

    #[test]
    fn group_or_other_access_is_a_unix_violation() {
        assert_eq!(unix_mode_violation(0o600), None);
        assert_eq!(unix_mode_violation(0o400), None);
        assert!(unix_mode_violation(0o640).unwrap().contains("640"));
        assert!(unix_mode_violation(0o604).is_some());
    }
}
