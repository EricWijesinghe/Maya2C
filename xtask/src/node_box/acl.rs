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

/// The commands that leave `path` readable and writable by `account` (the
/// account the node runs as) and SYSTEM only. Grants come before inheritance
/// is removed: removing inheritance first would strip the operator's own
/// access and lock the key away. When the node runs as another account than
/// the `operator` running setup (the boot task runs it as SYSTEM), the
/// operator's own grant is removed last: the node refuses a passphrase file
/// that grants any account but its own, SYSTEM and Administrators
/// (`maya_signer::owner_only::check`). The operator keeps ownership, so can
/// still list and change the ACL.
pub fn key_acl_steps(os: Os, path: &str, account: &str, operator: &str) -> Vec<Step> {
    match os {
        Os::Windows => {
            let mut grant = vec![
                path.to_owned(),
                "/grant:r".to_owned(),
                format!("{account}:F"),
            ];
            if !is_system(account) {
                grant.extend(strings(&["/grant:r", "SYSTEM:F"]));
            }
            let mut steps = vec![
                Step {
                    program: "icacls",
                    args: grant,
                },
                Step {
                    program: "icacls",
                    args: [vec![path.to_owned()], strings(&["/inheritance:r"])].concat(),
                },
            ];
            if !same_account(operator, account) {
                steps.push(Step {
                    program: "icacls",
                    args: vec![path.to_owned(), "/remove:g".to_owned(), operator.to_owned()],
                });
            }
            steps
        }
        Os::Unix => vec![Step {
            program: "chmod",
            args: vec!["600".to_owned(), path.to_owned()],
        }],
    }
}

/// The last part of `DOMAIN\name`, lower-cased: how account names compare.
fn short_name(principal: &str) -> String {
    principal
        .rsplit('\\')
        .next()
        .unwrap_or(principal)
        .to_ascii_lowercase()
}

fn same_account(a: &str, b: &str) -> bool {
    short_name(a) == short_name(b)
}

fn is_system(principal: &str) -> bool {
    short_name(principal) == "system"
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

/// Whether a principal may touch a key: the account the node runs as, or
/// SYSTEM. `DOMAIN\name` and `COMPUTER\name` compare by their last part.
pub fn is_allowed(principal: &str, account: &str) -> bool {
    same_account(principal, account) || is_system(principal)
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
            // An unreadable ACL lists no principals; that must not pass as clean.
            if !out.status.success() {
                return Err(format!(
                    "icacls {shown}: {} {}",
                    out.status,
                    String::from_utf8_lossy(&out.stdout).trim()
                ));
            }
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
        let steps = key_acl_steps(Os::Windows, PATH, "EricW", "EricW");
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0].args,
            [PATH, "/grant:r", "EricW:F", "/grant:r", "SYSTEM:F"]
        );
        assert_eq!(steps[1].args, [PATH, "/inheritance:r"]);
        assert!(steps.iter().all(|s| s.program == "icacls"));
    }

    #[test]
    fn a_node_run_as_system_gets_system_only_and_the_operator_grant_removed() {
        let steps = key_acl_steps(Os::Windows, PATH, "SYSTEM", "EricW");
        let args: Vec<_> = steps.iter().map(|s| s.args.clone()).collect();
        assert_eq!(
            args,
            [
                vec![PATH, "/grant:r", "SYSTEM:F"],
                vec![PATH, "/inheritance:r"],
                vec![PATH, "/remove:g", "EricW"],
            ]
        );
    }

    /// Setup's real steps on a real file, then the node's own passphrase rule
    /// as the boot task's account (SYSTEM, S-1-5-18). Before the fix setup left
    /// the operator's grant, which the node refused as another account.
    #[cfg(windows)]
    #[test]
    fn setup_acl_passes_the_node_check_when_the_node_runs_as_system() {
        let operator = std::env::var("USERNAME").unwrap();
        let path =
            std::env::temp_dir().join(format!("maya2c-acl-{}.passphrase", std::process::id()));
        std::fs::write(&path, "x").unwrap();
        let shown = path.display().to_string();
        let sddl = || {
            let out = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "(Get-Acl -LiteralPath $env:P).Sddl",
                ])
                .env("P", &path)
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        for step in key_acl_steps(Os::Windows, &shown, &operator, &operator) {
            run_step(&step).unwrap();
        }
        let old_rule = maya_signer::owner_only::others_granted(&sddl(), "S-1-5-18");
        for step in key_acl_steps(Os::Windows, &shown, "SYSTEM", &operator) {
            run_step(&step).unwrap();
        }
        let new_rule = maya_signer::owner_only::others_granted(&sddl(), "S-1-5-18");
        let doctor = current_violations(Os::Windows, &path, "SYSTEM").unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            old_rule.len(),
            1,
            "the operator-and-SYSTEM ACL should be refused: {old_rule:?}"
        );
        assert!(new_rule.is_empty(), "node would refuse: {new_rule:?}");
        assert!(doctor.is_empty(), "doctor would fail: {doctor:?}");
    }

    #[test]
    fn unix_sets_mode_0600_on_the_key() {
        let steps = key_acl_steps(Os::Unix, "/home/op/validator.key", "op", "op");
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
