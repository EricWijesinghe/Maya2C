//! Checks the Hub makes before it runs the miner or hands it a key file.
//!
//! Settings live in a plain file on disk, so every field in them is attacker
//! input. The miner is only ever the copy shipped beside this executable, and
//! only when its SHA-256 matches the release checksums compiled into this
//! build. The key file must stay inside the Hub's data folder.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Release `sha256sum` lines for this build, from `MAYA2C_MINER_SHA256SUMS`
/// at build time (build.rs). Empty when unset, which refuses every miner.
pub const PINNED_SUMS: &str = include_str!(concat!(env!("OUT_DIR"), "/miner-sha256sums.txt"));

#[derive(Debug, PartialEq, Eq)]
pub enum GuardError {
    /// The settings name a program that is not the Maya2C miner.
    UnknownProgram(String),
    /// The miner resolves outside the Hub's install folder (a link included).
    OutsideInstall(PathBuf),
    /// No miner file where the release bundle puts it.
    MinerMissing(PathBuf),
    /// This build's checksums do not list the miner.
    Unpinned(String),
    /// The miner on disk is not the released file.
    HashMismatch { expected: String, actual: String },
    /// The key file setting is empty or names no file.
    KeyNameMissing,
    /// The key file folder does not exist.
    KeyFolderMissing(PathBuf),
    /// The key file resolves outside the Hub's data folder (a link included).
    KeyOutsideDataDir(PathBuf),
    Io(String),
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownProgram(name) => write!(f, "{name} is not the Maya2C miner"),
            Self::OutsideInstall(path) => write!(
                f,
                "{} is outside the Hub's install folder; refusing to run it",
                path.display()
            ),
            Self::MinerMissing(path) => {
                write!(f, "miner not found at {} (reinstall the Hub)", path.display())
            }
            Self::Unpinned(name) => write!(
                f,
                "this Hub build has no checksum for {name}; refusing to run it"
            ),
            Self::HashMismatch { expected, actual } => write!(
                f,
                "miner checksum is {actual}, the release says {expected}; refusing to run it"
            ),
            Self::KeyNameMissing => write!(f, "choose a payout key file"),
            Self::KeyFolderMissing(path) => {
                write!(f, "key file folder {} does not exist", path.display())
            }
            Self::KeyOutsideDataDir(path) => write!(
                f,
                "key file {} must be inside the Hub's data folder",
                path.display()
            ),
            Self::Io(message) => write!(f, "could not check the miner: {message}"),
        }
    }
}

impl std::error::Error for GuardError {}

fn io(e: std::io::Error) -> GuardError {
    GuardError::Io(e.to_string())
}

/// The miner's file name on this platform.
#[must_use]
pub fn miner_file_name() -> String {
    format!("maya2c-gpu-miner{}", std::env::consts::EXE_SUFFIX)
}

/// The canonical folder the running Hub executable lives in.
///
/// # Errors
///
/// The executable path cannot be read.
pub fn install_dir() -> Result<PathBuf, GuardError> {
    let exe = std::env::current_exe().map_err(io)?;
    let dir = exe
        .parent()
        .ok_or_else(|| GuardError::Io("the Hub has no install folder".into()))?;
    dir.canonicalize().map_err(io)
}

fn file_name_of(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

/// Resolves the miner the settings name to the copy beside the Hub. A bare
/// name is taken from `install`; a path must already point into it.
///
/// # Errors
///
/// The name is not the miner's, the path leaves `install` (through `..`, an
/// absolute path, or a symlink or junction), or no file is there.
pub fn resolve_miner(requested: &str, install: &Path) -> Result<PathBuf, GuardError> {
    let req = Path::new(requested);
    let wanted = miner_file_name();
    let name = file_name_of(req)
        .filter(|n| *n == wanted)
        .ok_or_else(|| GuardError::UnknownProgram(requested.to_string()))?;
    let dir = install.canonicalize().map_err(io)?;
    if req.parent().is_some_and(|p| !p.as_os_str().is_empty()) {
        let given = req
            .canonicalize()
            .map_err(|_| GuardError::MinerMissing(req.to_path_buf()))?;
        if given.parent() != Some(dir.as_path()) {
            return Err(GuardError::OutsideInstall(given));
        }
    }
    let candidate = dir.join(name);
    let real = candidate
        .canonicalize()
        .map_err(|_| GuardError::MinerMissing(candidate.clone()))?;
    if real.parent() != Some(dir.as_path()) {
        return Err(GuardError::OutsideInstall(real));
    }
    if file_name_of(&real) != Some(wanted.as_str()) || !real.is_file() {
        return Err(GuardError::UnknownProgram(real.display().to_string()));
    }
    Ok(real)
}

/// Lowercase hex SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Parses `sha256sum` output: `<hex>  <name>` or `<hex> *<name>`.
#[must_use]
pub fn parse_sums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim().split_once(char::is_whitespace)?;
            let hash = hash.to_ascii_lowercase();
            let name = name.trim_start().trim_start_matches('*');
            let well_formed = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
            well_formed.then(|| (hash, name.to_string()))
        })
        .collect()
}

/// The one checksum `sums` lists for `file`.
///
/// # Errors
///
/// No entry, or more than one.
pub fn pinned_hash(sums: &str, file: &str) -> Result<String, GuardError> {
    let mut found = parse_sums(sums)
        .into_iter()
        .filter(|(_, name)| name == file)
        .map(|(hash, _)| hash);
    match (found.next(), found.next()) {
        (Some(hash), None) => Ok(hash),
        _ => Err(GuardError::Unpinned(file.to_string())),
    }
}

/// # Errors
///
/// The file cannot be read, or its digest differs from `expected`.
pub fn verify_hash(path: &Path, expected: &str) -> Result<(), GuardError> {
    let actual = sha256_hex(&fs::read(path).map_err(io)?);
    if actual == expected {
        Ok(())
    } else {
        Err(GuardError::HashMismatch {
            expected: expected.to_string(),
            actual,
        })
    }
}

/// The miner the Hub may run: the bundled copy whose SHA-256 matches `sums`.
///
/// # Errors
///
/// Any failure of [`resolve_miner`], [`pinned_hash`] or [`verify_hash`].
pub fn check_miner(requested: &str, install: &Path, sums: &str) -> Result<PathBuf, GuardError> {
    let path = resolve_miner(requested, install)?;
    let expected = pinned_hash(sums, &miner_file_name())?;
    verify_hash(&path, &expected)?;
    Ok(path)
}

/// The payout key file, which must sit inside `data`. A link there is
/// followed before the check, so a link out of the folder is refused.
///
/// # Errors
///
/// No file name, a missing folder, or a path that leaves `data`.
pub fn resolve_key_file(requested: &str, data: &Path) -> Result<PathBuf, GuardError> {
    let req = Path::new(requested.trim());
    let name = file_name_of(req).ok_or(GuardError::KeyNameMissing)?;
    fs::create_dir_all(data).map_err(io)?;
    let data = data.canonicalize().map_err(io)?;
    let full = if req.is_absolute() {
        req.to_path_buf()
    } else {
        data.join(req)
    };
    let parent = full
        .parent()
        .ok_or(GuardError::KeyNameMissing)?
        .canonicalize()
        .map_err(|_| GuardError::KeyFolderMissing(full.clone()))?;
    if !parent.starts_with(&data) {
        return Err(GuardError::KeyOutsideDataDir(parent.join(name)));
    }
    let target = parent.join(name);
    if fs::symlink_metadata(&target).is_ok() {
        let real = target.canonicalize().map_err(io)?;
        if !real.starts_with(&data) {
            return Err(GuardError::KeyOutsideDataDir(real));
        }
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the OS temp dir, unique per test name.
    fn scratch(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("maya-hub-guard-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch folder");
        dir
    }

    /// `base/install` holding a fake miner, and the sums that pin it.
    fn bundle(test: &str) -> (PathBuf, PathBuf, String) {
        let base = scratch(test);
        let install = base.join("install");
        fs::create_dir_all(&install).expect("install folder");
        let miner = install.join(miner_file_name());
        fs::write(&miner, b"fake miner").expect("write miner");
        let sums = format!("{}  {}\n", sha256_hex(b"fake miner"), miner_file_name());
        (base, install, sums)
    }

    #[test]
    fn sha256_matches_the_standard_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn bundled_miner_with_a_matching_hash_is_accepted() {
        let (_base, install, sums) = bundle("good");
        let path = check_miner(&miner_file_name(), &install, &sums).expect("accepted");
        assert_eq!(path.file_name().unwrap().to_str(), Some(miner_file_name().as_str()));
    }

    #[test]
    fn hash_mismatch_is_refused() {
        let (_base, install, _sums) = bundle("mismatch");
        let wrong = format!("{}  {}", "0".repeat(64), miner_file_name());
        let err = check_miner(&miner_file_name(), &install, &wrong).unwrap_err();
        assert!(matches!(err, GuardError::HashMismatch { .. }), "{err}");
    }

    #[test]
    fn miner_absent_from_the_checksums_is_refused() {
        let (_base, install, _sums) = bundle("unpinned");
        let err = check_miner(&miner_file_name(), &install, "").unwrap_err();
        assert_eq!(err, GuardError::Unpinned(miner_file_name()));
    }

    #[test]
    fn unknown_program_name_is_refused() {
        let (_base, install, sums) = bundle("unknown");
        let err = check_miner("evil.exe", &install, &sums).unwrap_err();
        assert_eq!(err, GuardError::UnknownProgram("evil.exe".into()));
    }

    #[test]
    fn path_traversal_out_of_the_install_folder_is_refused() {
        let (base, install, sums) = bundle("traversal");
        fs::write(base.join(miner_file_name()), b"fake miner").unwrap();
        let requested = install.join("..").join(miner_file_name());
        let err = check_miner(&requested.display().to_string(), &install, &sums).unwrap_err();
        assert!(matches!(err, GuardError::OutsideInstall(_)), "{err}");
    }

    #[test]
    fn absolute_path_outside_the_install_folder_is_refused() {
        let (base, install, sums) = bundle("absolute");
        let outside = base.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join(miner_file_name()), b"fake miner").unwrap();
        let requested = outside.join(miner_file_name());
        let err = check_miner(&requested.display().to_string(), &install, &sums).unwrap_err();
        assert!(matches!(err, GuardError::OutsideInstall(_)), "{err}");
    }

    #[test]
    fn symlink_escape_out_of_the_install_folder_is_refused() {
        let (base, install, sums) = bundle("symlink");
        let outside = base.join("outside");
        fs::create_dir_all(&outside).unwrap();
        let target = outside.join(miner_file_name());
        fs::write(&target, b"fake miner").unwrap();
        let link = install.join(miner_file_name());
        fs::remove_file(&link).unwrap();
        make_file_link(&target, &link);
        let err = check_miner(&miner_file_name(), &install, &sums).unwrap_err();
        assert!(matches!(err, GuardError::OutsideInstall(_)), "{err}");
    }

    #[cfg(unix)]
    fn make_file_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).expect("symlink");
    }

    #[cfg(windows)]
    fn make_file_link(target: &Path, link: &Path) {
        std::os::windows::fs::symlink_file(target, link).expect("symlink (Developer Mode)");
    }

    #[test]
    fn key_file_inside_the_data_folder_is_accepted() {
        let base = scratch("key-good");
        let data = base.join("data");
        let path = resolve_key_file("miner.key", &data).expect("accepted");
        assert!(path.starts_with(data.canonicalize().unwrap()));
    }

    #[test]
    fn key_file_path_out_of_the_data_folder_is_refused() {
        let base = scratch("key-traversal");
        let data = base.join("data");
        fs::create_dir_all(&data).unwrap();
        let err = resolve_key_file("../stolen.key", &data).unwrap_err();
        assert!(matches!(err, GuardError::KeyOutsideDataDir(_)), "{err}");
    }

    #[test]
    fn absolute_key_file_outside_the_data_folder_is_refused() {
        let base = scratch("key-absolute");
        let data = base.join("data");
        fs::create_dir_all(&data).unwrap();
        let outside = base.join("elsewhere.key");
        let err = resolve_key_file(&outside.display().to_string(), &data).unwrap_err();
        assert!(matches!(err, GuardError::KeyOutsideDataDir(_)), "{err}");
    }

    #[test]
    fn key_file_symlink_escape_is_refused() {
        let base = scratch("key-symlink");
        let data = base.join("data");
        fs::create_dir_all(&data).unwrap();
        let outside = base.join("victim.key");
        fs::write(&outside, b"secret").unwrap();
        make_file_link(&outside, &data.join("miner.key"));
        let err = resolve_key_file("miner.key", &data).unwrap_err();
        assert!(matches!(err, GuardError::KeyOutsideDataDir(_)), "{err}");
    }
}
