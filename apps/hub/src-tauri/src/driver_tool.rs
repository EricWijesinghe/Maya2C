//! Where the Hub finds `nvidia-smi` (ADR-137 S-8, threat model T-4).
//!
//! Only by an absolute system path, never by a `PATH` search: on Windows a
//! bare program name is looked up in the Hub's own folder first, so a file
//! planted beside the Hub would run. A candidate that is missing or fails its
//! check is not run, and there is no fallback to the bare name.
//!
//! GPU mining fails closed on it: with no trusted tool there is no
//! temperature reading and so no thermal stop, and [`require_telemetry`]
//! refuses the start. A per-machine override (Settings → Driver tool) must
//! pass checks as strict as the system path's, and replaces it: a refused
//! override refuses mining rather than falling back to anything.

use std::path::{Path, PathBuf};
use std::sync::{PoisonError, RwLock};

/// The override the webview sent with `driver_tool_set`. Kept as set and
/// checked again on every use, so a file swapped after it was set is caught.
static OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

#[cfg(windows)]
const OVERRIDE_RULE: &str = "It must be a file under Program Files or System32.";
#[cfg(not(windows))]
const OVERRIDE_RULE: &str = "It must be owned by root, and neither it nor any folder above it may be writable by group or others.";

/// The candidate paths, all absolute and all in folders only an
/// administrator or root can write.
#[must_use]
pub fn candidates() -> Vec<PathBuf> {
    // shortcut: System32 only. Pre-DCH drivers put the tool under
    // `Program Files\NVIDIA Corporation\NVSMI`; add it (via the known-folder
    // API, not an environment variable) if a supported driver still does.
    #[cfg(windows)]
    {
        system_dir()
            .map(|d| vec![d.join("nvidia-smi.exe")])
            .unwrap_or_default()
    }
    #[cfg(not(windows))]
    {
        vec![PathBuf::from("/usr/bin/nvidia-smi")]
    }
}

/// The Windows system folder, from the API rather than `%SystemRoot%`: an
/// environment variable is whatever started the Hub says it is.
#[cfg(windows)]
fn system_dir() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    const MAX_PATH_UNITS: usize = 260;
    let mut buf = [0u16; MAX_PATH_UNITS];
    // SAFETY: the slice is a valid, writable local buffer; the call writes at
    // most `buf.len()` units and returns how many it wrote (0 on failure, or
    // more than the length when the buffer is too small, refused below).
    let len = unsafe { GetSystemDirectoryW(Some(&mut buf)) } as usize;
    (len > 0 && len < buf.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..len])))
}

/// True for a file mode and owner only root can change: owned by root, not
/// writable by group or others.
#[cfg(unix)]
#[must_use]
pub const fn root_only(mode: u32, uid: u32) -> bool {
    const GROUP_OR_OTHER_WRITE: u32 = 0o022;
    uid == 0 && mode & GROUP_OR_OTHER_WRITE == 0
}

/// The path to run for one candidate, or `None` when it fails its check.
fn trusted(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // Distribution packages often reach the binary through symlinks. The
        // resolved file is what runs, so it and every folder above it must be
        // root-only; the symlinks are not followed again at run time.
        let real = std::fs::canonicalize(path).ok()?;
        let ok = |p: &Path| std::fs::metadata(p).is_ok_and(|m| root_only(m.mode(), m.uid()));
        let is_file = std::fs::metadata(&real).is_ok_and(|m| m.is_file());
        (is_file && real.ancestors().all(ok)).then_some(real)
    }
    #[cfg(not(unix))]
    {
        // System32 is writable only by TrustedInstaller and administrators;
        // a reparse point there is refused so the path that runs is the
        // path that was checked.
        std::fs::symlink_metadata(path)
            .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
            .then(|| path.to_path_buf())
    }
}

/// The Windows Program Files folder, from the known-folder API rather than
/// `%ProgramFiles%`, for the same reason as [`system_dir`].
#[cfg(windows)]
fn program_files_dir() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    // SAFETY: the GUID is a static the call only reads; on success it returns
    // a NUL-terminated string the shell allocated, read once into an owned
    // `OsString` and then freed with `CoTaskMemFree`, as the API requires.
    unsafe {
        let raw = SHGetKnownFolderPath(&FOLDERID_ProgramFiles, KF_FLAG_DEFAULT, None).ok()?;
        let path = PathBuf::from(std::ffi::OsString::from_wide(raw.as_wide()));
        CoTaskMemFree(Some(raw.0.cast_const().cast()));
        Some(path)
    }
}

/// The path to run for an override, or `None` when it fails its check.
#[cfg(unix)]
fn trusted_override(path: &Path) -> Option<PathBuf> {
    trusted(path)
}

/// The path to run for an override, or `None` when it fails its check: an
/// existing file whose resolved path lies under Program Files or System32,
/// both folders found through the Windows API.
// shortcut: the folder is trusted, not each subfolder's ACL; an installer
// that loosens a subfolder's ACL would pass. Check the ACL (GetNamedSecurityInfoW)
// if a supported driver ships the tool in such a folder.
#[cfg(windows)]
fn trusted_override(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let real = std::fs::canonicalize(path).ok()?;
    let is_file = std::fs::symlink_metadata(&real).is_ok_and(|m| m.is_file());
    let under = |root: Option<PathBuf>| {
        root.and_then(|r| std::fs::canonicalize(r).ok())
            .is_some_and(|r| real.starts_with(r))
    };
    (is_file && (under(system_dir()) || under(program_files_dir()))).then_some(real)
}

#[cfg(not(any(unix, windows)))]
fn trusted_override(_path: &Path) -> Option<PathBuf> {
    None
}

/// The first candidate that passes its check.
fn resolve_from(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find_map(|p| trusted(p))
}

/// The override when one is set, and only it; otherwise the system paths.
fn resolve(over: Option<&Path>, system: &[PathBuf]) -> Option<PathBuf> {
    over.map_or_else(|| resolve_from(system), trusted_override)
}

fn current_override() -> Option<PathBuf> {
    OVERRIDE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Sets or clears (`None`) the per-machine override. It is stored even when
/// it fails its check, so mining stays refused instead of quietly using
/// another tool.
///
/// # Errors
///
/// The override fails its check; the message says what the rule is.
pub fn set_override(path: Option<PathBuf>) -> Result<(), String> {
    let refused = path
        .as_deref()
        .filter(|p| trusted_override(p).is_none())
        .map(refused_override);
    *OVERRIDE.write().unwrap_or_else(PoisonError::into_inner) = path;
    refused.map_or(Ok(()), Err)
}

/// `nvidia-smi` by absolute path, or `None`. Never the bare name.
#[must_use]
pub fn nvidia_smi() -> Option<PathBuf> {
    resolve(current_override().as_deref(), &candidates())
}

/// The trusted tool GPU mining needs, or why mining may not start.
///
/// # Errors
///
/// No tool passes its check: a message with the fix, for the status line.
pub fn require_telemetry() -> Result<PathBuf, String> {
    require_from(current_override().as_deref(), &candidates())
}

fn require_from(over: Option<&Path>, system: &[PathBuf]) -> Result<PathBuf, String> {
    resolve(over, system).ok_or_else(|| match over {
        Some(p) => refused_override(p),
        None => no_tool(system),
    })
}

fn refused_override(path: &Path) -> String {
    format!(
        "GPU mining not started: the driver tool set in Settings ({}) failed its check. {OVERRIDE_RULE} Fix the path, or clear it to use the system one.",
        path.display()
    )
}

fn no_tool(system: &[PathBuf]) -> String {
    let expected = system.first().map_or_else(
        || "its system folder".to_string(),
        |p| p.display().to_string(),
    );
    format!(
        "GPU mining not started: the Hub found no trusted GPU temperature source, so it could not stop an overheating card. Install or update the NVIDIA driver, which puts nvidia-smi at {expected}, or set Settings → Driver tool to where your driver put it. CPU mining still works."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_tools_are_resolved_by_absolute_path_never_by_search() {
        assert!(candidates().iter().all(|p| p.is_absolute()));
        assert_eq!(
            resolve_from(&[PathBuf::from("nvidia-smi")]),
            None,
            "a bare name would be a search"
        );
        let missing = std::env::temp_dir()
            .join("hub-no-such-dir")
            .join("nvidia-smi");
        assert_eq!(
            resolve_from(&[missing]),
            None,
            "a missing tool is no tool, not a fallback to PATH"
        );
        if let Some(found) = nvidia_smi() {
            assert!(found.is_absolute());
        }
    }

    #[test]
    fn a_planted_driver_tool_beside_the_hub_is_never_run() {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf));
        let cwd = std::env::current_dir().ok();
        for c in candidates() {
            let dir = c.parent().map(Path::to_path_buf);
            assert!(dir.is_some());
            assert_ne!(dir, exe_dir, "{} sits beside the Hub", c.display());
            assert_ne!(dir, cwd, "{} sits in the working folder", c.display());
        }
        // A file planted in a user-writable folder fails the check even when
        // named by its absolute path.
        let planted_dir = std::env::temp_dir().join(format!("hub-planted-{}", std::process::id()));
        std::fs::create_dir_all(&planted_dir).expect("temp dir");
        let planted = planted_dir.join("nvidia-smi");
        std::fs::write(&planted, "#!/bin/sh\n").expect("plant");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // World-writable, so the check refuses it even when CI runs as root.
            std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o777))
                .expect("chmod");
            assert_eq!(
                resolve_from(std::slice::from_ref(&planted)),
                None,
                "a user-writable tool"
            );
        }
        #[cfg(not(unix))]
        assert!(
            !candidates().contains(&planted),
            "a temp-folder file is never a candidate"
        );
        let _ = std::fs::remove_dir_all(&planted_dir);
    }

    fn missing_system_tool() -> Vec<PathBuf> {
        vec![
            std::env::temp_dir()
                .join("hub-no-such-dir")
                .join("nvidia-smi"),
        ]
    }

    /// A real file that passes the override check on this OS, without a GPU.
    fn trusted_stand_in() -> PathBuf {
        #[cfg(windows)]
        {
            system_dir().expect("system folder").join("cmd.exe")
        }
        #[cfg(not(windows))]
        {
            PathBuf::from("/bin/sh")
        }
    }

    #[test]
    fn no_trusted_tool_refuses_gpu_mining_with_the_fix() {
        let system = missing_system_tool();
        let err = require_from(None, &system).expect_err("no tool, no mining");
        assert!(err.starts_with("GPU mining not started"), "{err}");
        assert!(
            err.contains(&system[0].display().to_string()),
            "names where the tool belongs: {err}"
        );
        assert!(err.contains("Settings"), "names the override: {err}");
    }

    #[test]
    fn an_override_that_passes_the_checks_allows_gpu_mining() {
        let found = require_from(Some(&trusted_stand_in()), &missing_system_tool())
            .expect("a root-owned / system-folder override is trusted");
        assert!(found.is_absolute());
    }

    #[test]
    fn an_override_that_fails_the_checks_refuses_gpu_mining() {
        let dir = std::env::temp_dir().join(format!("hub-override-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let planted = dir.join("nvidia-smi");
        std::fs::write(&planted, "#!/bin/sh\n").expect("plant");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o777))
                .expect("chmod");
        }
        // A refused override never falls back, even to a system tool that passes.
        let system = vec![trusted_stand_in()];
        for bad in [planted.clone(), PathBuf::from("nvidia-smi")] {
            let err = require_from(Some(&bad), &system).expect_err("refused override");
            assert!(err.contains("failed its check"), "{err}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn program_files_comes_from_the_known_folder_api() {
        let dir = program_files_dir().expect("Program Files");
        assert!(dir.is_absolute() && dir.is_dir(), "{}", dir.display());
    }

    #[cfg(unix)]
    #[test]
    fn only_a_root_owned_unwritable_tool_is_trusted() {
        assert!(root_only(0o100_755, 0));
        assert!(!root_only(0o100_775, 0), "group-writable");
        assert!(!root_only(0o100_757, 0), "world-writable");
        assert!(!root_only(0o100_755, 1000), "owned by a user");
    }
}
