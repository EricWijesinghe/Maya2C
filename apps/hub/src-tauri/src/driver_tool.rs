//! Where the Hub finds `nvidia-smi` (ADR-137 S-8, threat model T-4).
//!
//! Only by an absolute system path, never by a `PATH` search: on Windows a
//! bare program name is looked up in the Hub's own folder first, so a file
//! planted beside the Hub would run. A candidate that is missing or fails its
//! check is not run, and there is no fallback to the bare name.

use std::path::{Path, PathBuf};

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

/// The first candidate that passes its check.
fn resolve_from(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find_map(|p| trusted(p))
}

/// `nvidia-smi` by absolute path, or `None`. Never the bare name.
#[must_use]
pub fn nvidia_smi() -> Option<PathBuf> {
    resolve_from(&candidates())
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
                resolve_from(&[planted.clone()]),
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

    #[cfg(unix)]
    #[test]
    fn only_a_root_owned_unwritable_tool_is_trusted() {
        assert!(root_only(0o100_755, 0));
        assert!(!root_only(0o100_775, 0), "group-writable");
        assert!(!root_only(0o100_757, 0), "world-writable");
        assert!(!root_only(0o100_755, 1000), "owned by a user");
    }
}
