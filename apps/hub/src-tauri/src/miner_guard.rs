//! Checks the Hub makes before it runs the miner or hands it a key file.
//!
//! Settings live in a plain file on disk, so every field in them is attacker
//! input. The miner is only ever the copy shipped beside this executable, and
//! only when its SHA-256 matches the release checksums compiled into this
//! build. The key file must stay inside the Hub's data folder.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
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
    HashMismatch {
        expected: String,
        actual: String,
    },
    /// The key file setting is empty or names no file.
    KeyNameMissing,
    /// The key file folder does not exist.
    KeyFolderMissing(PathBuf),
    /// The key file resolves outside the Hub's data folder (a link included).
    KeyOutsideDataDir(PathBuf),
    /// The key file is readable or writable by someone other than its owner.
    KeyOpenToOthers(PathBuf),
    /// The key file belongs to another user.
    KeyNotOwned(PathBuf),
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
                write!(
                    f,
                    "miner not found at {} (reinstall the Hub)",
                    path.display()
                )
            }
            Self::Unpinned(name) => write!(
                f,
                "this Hub build has no miner checksum for {name} (build it with cargo xtask hub-bundle); refusing to run it"
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
            #[cfg(unix)]
            Self::KeyOpenToOthers(path) => write!(
                f,
                "key file {0} is readable or writable by other users; fix it with: chmod 600 {0}",
                path.display()
            ),
            #[cfg(not(unix))]
            Self::KeyOpenToOthers(path) => write!(
                f,
                "key file {} is readable or writable by accounts other than yours, SYSTEM and Administrators; remove those permissions",
                path.display()
            ),
            Self::KeyNotOwned(path) => write!(
                f,
                "key file {} belongs to another user; copy it into your own account, then run chmod 600 on the copy",
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

/// The miner the Hub may run, held until its child exits.
///
/// On Windows `path` is the bundled file and `file` takes `FILE_SHARE_READ`
/// only, so a writer or deleter cannot open it while this exists.
/// On Unix `path` is a private copy of the hashed bytes ([`stage_copy`]), so
/// nothing done to the install folder after the check reaches the run.
#[derive(Debug)]
pub struct PinnedMiner {
    pub path: PathBuf,
    pub file: File,
    #[cfg(unix)]
    _scratch: Scratch,
}

/// The private folder holding the Unix copy; removed when the miner is dropped.
#[cfg(unix)]
#[derive(Debug)]
struct Scratch(PathBuf);

#[cfg(unix)]
impl Drop for Scratch {
    fn drop(&mut self) {
        // Drop cannot report. The folder is ours and 0700, so a leftover is harmless.
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Writes the bytes that were hashed into a fresh 0700 folder in the temp dir,
/// then re-hashes the copy. `create` refuses a name that already exists, so a
/// folder someone planted there is an error, never reused.
///
/// shortcut: a fork on another thread during the copy can hold the write
/// handle and make the exec fail with ETXTBSY; the start fails closed, retry it.
#[cfg(unix)]
fn stage_copy(bytes: &[u8], expected: &str) -> Result<(PathBuf, File, Scratch), GuardError> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("maya-hub-miner-{}-{nonce}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&dir).map_err(io)?;
    let scratch = Scratch(dir.clone());
    let copy = dir.join(miner_file_name());
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&copy)
        .map_err(io)?;
    out.write_all(bytes).map_err(io)?;
    drop(out);
    let file = File::open(&copy).map_err(io)?;
    let mut again = Vec::new();
    (&file).read_to_end(&mut again).map_err(io)?;
    let actual = sha256_hex(&again);
    if actual != expected {
        return Err(GuardError::HashMismatch {
            expected: expected.to_string(),
            actual,
        });
    }
    Ok((copy, file, scratch))
}

/// Builds the handle the Hub runs. Windows keeps the locked install file.
#[cfg(unix)]
fn pin(
    _path: PathBuf,
    _file: File,
    bytes: &[u8],
    expected: &str,
) -> Result<PinnedMiner, GuardError> {
    let (path, file, scratch) = stage_copy(bytes, expected)?;
    Ok(PinnedMiner {
        path,
        file,
        _scratch: scratch,
    })
}

#[cfg(not(unix))]
fn pin(
    path: PathBuf,
    file: File,
    _bytes: &[u8],
    _expected: &str,
) -> Result<PinnedMiner, GuardError> {
    Ok(PinnedMiner { path, file })
}

/// Opens the miner for reading only, sharing reads and nothing else on Windows.
fn open_read_only(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ (0x1): other readers may open it; writers and deleters may not.
        options.share_mode(0x1);
    }
    options.open(path)
}

/// The miner the Hub may run: the bundled copy whose SHA-256 matches `sums`.
/// On Unix the returned path is a private copy of exactly the hashed bytes.
///
/// # Errors
///
/// Any failure of [`resolve_miner`], [`pinned_hash`], opening or reading, a
/// hash mismatch, or [`GuardError::OutsideInstall`] if the path moved on open.
pub fn check_miner(requested: &str, install: &Path, sums: &str) -> Result<PinnedMiner, GuardError> {
    let path = resolve_miner(requested, install)?;
    let expected = pinned_hash(sums, &miner_file_name())?;
    let mut file = open_read_only(&path).map_err(io)?;
    let reresolved = path.canonicalize().map_err(io)?;
    if reresolved != path {
        return Err(GuardError::OutsideInstall(reresolved));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(io)?;
    let actual = sha256_hex(&bytes);
    if actual != expected {
        return Err(GuardError::HashMismatch { expected, actual });
    }
    pin(path, file, &bytes, &expected)
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
        check_key_permissions(&real)?;
    }
    Ok(target)
}

/// Other users' access bits (group and other). Any set means someone else can
/// read or write the key.
#[cfg(unix)]
const OTHERS_ACCESS_BITS: u32 = 0o077;

/// Refuses a key file another user can read or write, or one this user does not own.
#[cfg(unix)]
fn check_key_permissions(path: &Path) -> Result<(), GuardError> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).map_err(io)?;
    let me = rustix::process::getuid().as_raw();
    check_key_mode(path, meta.mode(), meta.uid(), me)
}

#[cfg(unix)]
fn check_key_mode(path: &Path, mode: u32, owner: u32, me: u32) -> Result<(), GuardError> {
    if owner != me {
        return Err(GuardError::KeyNotOwned(path.to_path_buf()));
    }
    if mode & OTHERS_ACCESS_BITS != 0 {
        return Err(GuardError::KeyOpenToOthers(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(windows)]
fn check_key_permissions(path: &Path) -> Result<(), GuardError> {
    match crate::key_acl::others_can_access(path) {
        Ok(false) => Ok(()),
        Ok(true) => Err(GuardError::KeyOpenToOthers(path.to_path_buf())),
        Err(message) => Err(GuardError::Io(message)),
    }
}

/// Fails closed where neither mode bits nor a DACL can be read.
#[cfg(not(any(unix, windows)))]
fn check_key_permissions(_path: &Path) -> Result<(), GuardError> {
    Err(GuardError::Io(
        "this platform cannot check key file permissions".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the OS temp dir, unique per test name.
    fn scratch(test: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("maya-hub-guard-{test}-{}", std::process::id()));
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
        let pinned = check_miner(&miner_file_name(), &install, &sums).expect("accepted");
        assert_eq!(
            pinned.path.file_name().unwrap().to_str(),
            Some(miner_file_name().as_str())
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_writer_cannot_open_the_miner_while_the_guard_holds_it() {
        const ERROR_SHARING_VIOLATION: i32 = 32;
        let (_base, install, sums) = bundle("writer");
        let pinned = check_miner(&miner_file_name(), &install, &sums).expect("accepted");
        let write = OpenOptions::new().write(true).open(&pinned.path);
        assert_eq!(
            write.expect_err("a writer must be refused").raw_os_error(),
            Some(ERROR_SHARING_VIOLATION)
        );
        assert!(
            fs::remove_file(&pinned.path).is_err(),
            "delete must be refused"
        );
        drop(pinned);
        OpenOptions::new()
            .write(true)
            .open(install.join(miner_file_name()))
            .expect("writable once the guard lets go");
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

    #[cfg(unix)]
    #[test]
    fn a_swapped_miner_after_the_check_does_not_change_what_runs() {
        let (_base, install, sums) = bundle("swap");
        let pinned = check_miner(&miner_file_name(), &install, &sums).expect("accepted");
        // Rewrite the install copy after the check, by rename and in place.
        let shipped = install.join(miner_file_name());
        fs::remove_file(&shipped).unwrap();
        fs::write(&shipped, b"evil miner").unwrap();
        assert_ne!(
            pinned.path, shipped,
            "the run must not use the install file"
        );
        assert_eq!(fs::read(&pinned.path).unwrap(), b"fake miner");
    }

    #[cfg(unix)]
    #[test]
    fn the_private_miner_copy_is_0700_and_removed_on_drop() {
        use std::os::unix::fs::MetadataExt;
        let (_base, install, sums) = bundle("cleanup");
        let pinned = check_miner(&miner_file_name(), &install, &sums).expect("accepted");
        let dir = pinned.path.parent().unwrap().to_path_buf();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&pinned.path).unwrap().mode() & 0o777, 0o700);
        drop(pinned);
        assert!(!dir.exists(), "the copy folder must go with the miner");
    }

    #[cfg(unix)]
    #[test]
    fn key_file_readable_by_others_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("key-open");
        let data = base.join("data");
        fs::create_dir_all(&data).unwrap();
        let key = data.join("miner.key");
        fs::write(&key, b"secret").unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
        let err = resolve_key_file("miner.key", &data).unwrap_err();
        assert!(matches!(err, GuardError::KeyOpenToOthers(_)), "{err}");
        assert!(err.to_string().contains("chmod 600"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn key_file_group_readable_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("key-group");
        let data = base.join("data");
        fs::create_dir_all(&data).unwrap();
        let key = data.join("miner.key");
        fs::write(&key, b"secret").unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o640)).unwrap();
        let err = resolve_key_file("miner.key", &data).unwrap_err();
        assert!(matches!(err, GuardError::KeyOpenToOthers(_)), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_key_file_is_accepted() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("key-private");
        let data = base.join("data");
        fs::create_dir_all(&data).unwrap();
        let key = data.join("miner.key");
        fs::write(&key, b"secret").unwrap();
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        let path = resolve_key_file("miner.key", &data).expect("accepted");
        assert_eq!(path, data.canonicalize().unwrap().join("miner.key"));
    }

    #[cfg(unix)]
    #[test]
    fn key_file_of_another_user_is_refused() {
        let path = Path::new("/data/miner.key");
        let err = check_key_mode(path, 0o100_600, 0, 1000).unwrap_err();
        assert_eq!(err, GuardError::KeyNotOwned(path.to_path_buf()));
        assert!(check_key_mode(path, 0o100_600, 1000, 1000).is_ok());
    }

    /// The miner hub-bundle shipped must pass the pin this build compiled in,
    /// and a copy with one byte flipped must not. CI runs it after
    /// `cargo xtask hub-bundle` (release-binaries.yml); `#[ignore]` keeps a
    /// plain `cargo test` from needing the sidecar, and a missing variable
    /// fails the test rather than passing it.
    #[test]
    #[ignore = "CI: needs MAYA2C_SHIPPED_MINER and MAYA2C_MINER_SHA256SUMS"]
    fn shipped_miner_passes_its_pin_and_a_flipped_byte_fails() {
        let shipped = std::env::var_os("MAYA2C_SHIPPED_MINER")
            .expect("set MAYA2C_SHIPPED_MINER to the bundled miner");
        let bytes = fs::read(&shipped).expect("read the bundled miner");
        assert!(!bytes.is_empty(), "the bundled miner is empty");
        let base = scratch("shipped");
        let install = base.join("install");
        fs::create_dir_all(&install).expect("install folder");
        let miner = install.join(miner_file_name());
        fs::write(&miner, &bytes).expect("copy the bundled miner");
        // The temporary handle drops at the end of this statement, before the rewrite below.
        check_miner(&miner_file_name(), &install, PINNED_SUMS).expect("bundled miner accepted");
        let mut flipped = bytes;
        flipped[0] ^= 0x01;
        fs::write(&miner, &flipped).expect("write the altered copy");
        let err = check_miner(&miner_file_name(), &install, PINNED_SUMS).unwrap_err();
        assert!(matches!(err, GuardError::HashMismatch { .. }), "{err}");
    }
}
