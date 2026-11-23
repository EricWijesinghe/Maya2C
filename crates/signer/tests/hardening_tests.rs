//! Regression tests for signer hardening: passphrase file modes, keystore
//! creation and parsing bounds, and slashing-database compaction and import.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_crypto_pq::suite::MasterSeed;
use maya_signer::keystore::{self, KeystoreError};
use maya_signer::protection::{
    DbError, FORMAT, Interchange, Kind, Metadata, Refusal, Signed, SlashingDb, ValidatorData,
};
#[cfg(unix)]
use maya_signer::sealed;
use tempfile::TempDir;

fn seed(b: u8) -> MasterSeed {
    MasterSeed::from_bytes([b; 32])
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
#[test]
fn a_passphrase_file_readable_by_others_is_refused() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pass");
    std::fs::write(&path, "correct horse battery\n").unwrap();

    set_mode(&path, 0o644);
    assert!(
        sealed::read_passphrase_file(&path).is_err(),
        "a group- or world-readable passphrase file was accepted"
    );

    set_mode(&path, 0o600);
    let read = sealed::read_passphrase_file(&path).unwrap();
    assert_eq!(read.as_slice(), b"correct horse battery");
}

#[test]
fn create_refuses_a_passphrase_below_the_minimum() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("key.json");
    let refused = keystore::create(&path, &seed(1), b"short");
    assert!(matches!(refused, Err(KeystoreError::Format(_))));
    assert!(
        !path.exists(),
        "a keystore was written under a weak passphrase"
    );
}

#[test]
fn create_refuses_to_overwrite_an_existing_keystore() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("key.json");
    keystore::create(&path, &seed(1), b"correct horse").unwrap();
    let before = std::fs::read(&path).unwrap();
    let again = keystore::create(&path, &seed(2), b"correct horse");
    assert!(matches!(again, Err(KeystoreError::Io(_))));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn a_new_keystore_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("key.json");
    keystore::create(&path, &seed(1), b"correct horse").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

/// Writes a keystore, rewrites one JSON field, and returns the path.
fn tampered(dir: &TempDir, field: &str, value: serde_json::Value) -> std::path::PathBuf {
    let path = dir.path().join("key.json");
    keystore::create(&path, &seed(1), b"correct horse").unwrap();
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    doc[field] = value;
    std::fs::write(&path, doc.to_string()).unwrap();
    path
}

#[test]
fn open_does_not_panic_on_a_multi_byte_character_in_hex() {
    let dir = TempDir::new().unwrap();
    // Four bytes, so the length check passes and the pair at 0..2 splits a character.
    let path = tampered(&dir, "salt", serde_json::Value::String("a\u{e9}b".into()));
    assert!(matches!(
        keystore::open(&path, b"correct horse"),
        Err(KeystoreError::Format(_))
    ));
}

#[test]
fn open_refuses_kdf_memory_below_the_accepted_range() {
    let dir = TempDir::new().unwrap();
    // 4 MiB is a valid Argon2 setting but below the accepted range, so it must
    // be refused as a format error rather than run and fail to decrypt.
    let path = tampered(&dir, "kdf_memory_kib", serde_json::json!(4096));
    assert!(matches!(
        keystore::open(&path, b"correct horse"),
        Err(KeystoreError::Format(_))
    ));
}

#[test]
fn open_refuses_an_oversized_keystore_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("key.json");
    keystore::create(&path, &seed(1), b"correct horse").unwrap();
    // A valid keystore padded past the cap with trailing whitespace, which
    // JSON allows: only the size bound can refuse it.
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str(&" ".repeat(70 * 1024));
    std::fs::write(&path, text).unwrap();
    assert!(matches!(
        keystore::open(&path, b"correct horse"),
        Err(KeystoreError::Format(_))
    ));
}

#[test]
fn import_refuses_an_author_outside_the_committee() {
    let dir = TempDir::new().unwrap();
    let mut db = SlashingDb::open(&dir.path().join("protection.db")).unwrap();
    let pubkey = [7u8; 32];
    let hex_pubkey = "07".repeat(32);
    let file = Interchange {
        metadata: Metadata {
            interchange_format_version: FORMAT.into(),
        },
        data: vec![ValidatorData {
            pubkey: hex_pubkey,
            signed_vertices: vec![Signed {
                round: "5".into(),
                author: "1024".into(),
                signing_root: "00".repeat(32),
            }],
            signed_votes: vec![],
        }],
    };
    assert!(matches!(
        db.import(&file, &pubkey),
        Err(DbError::Refused(Refusal::OutOfRange { .. }))
    ));
}

#[test]
fn a_compacted_database_keeps_records_written_after_compaction() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("protection.db");
    {
        let mut db = SlashingDb::open(&path).unwrap();
        db.approve(Kind::Vote, 1, 0, [1; 32]).unwrap();
    }
    // One live record, 1100 lines: opening compacts the file.
    let line = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, line.repeat(1_100)).unwrap();

    let mut db = SlashingDb::open(&path).unwrap();
    db.approve(Kind::Vote, 2, 0, [2; 32]).unwrap();
    drop(db);

    let mut db = SlashingDb::open(&path).unwrap();
    assert!(
        matches!(
            db.approve(Kind::Vote, 2, 0, [3; 32]),
            Err(DbError::Refused(Refusal::Conflict { .. }))
        ),
        "a record written after compaction was lost on reopen"
    );
}

/// Windows (audit NEW-01): a folder that grants Everyone read on everything
/// created in it, the case a new file's inherited DACL gets wrong.
#[cfg(windows)]
fn everyone_readable_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    icacls(dir.path(), &["/grant", "*S-1-1-0:(OI)(CI)R"]);
    dir
}

#[cfg(windows)]
fn icacls(path: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(args)
        .output()
        .unwrap()
        .status;
    assert!(status.success(), "icacls {args:?}");
}

/// The file's DACL in SDDL, read independently of the code under test.
#[cfg(windows)]
fn sddl(path: &std::path::Path) -> String {
    let out = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-Acl -LiteralPath $env:P).Sddl",
        ])
        .env("P", path)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[cfg(windows)]
#[test]
fn a_new_keystore_in_a_folder_everyone_can_read_is_owner_only() {
    let dir = everyone_readable_dir();
    let path = dir.path().join("key.json");
    keystore::create(&path, &seed(1), b"correct horse").unwrap();
    let acl = sddl(&path);
    assert!(
        !acl.contains(";;;WD)"),
        "Everyone can read the new keystore: {acl}"
    );
    keystore::open(&path, b"correct horse").expect("the owner still opens it");
}

#[cfg(windows)]
#[test]
fn a_passphrase_file_everyone_can_read_is_refused_on_windows() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("pass");
    std::fs::write(&path, "correct horse battery\n").unwrap();
    let me = std::env::var("USERNAME").unwrap();
    icacls(
        &path,
        &[
            "/inheritance:r",
            "/grant:r",
            &format!("{me}:F"),
            "*S-1-1-0:R",
        ],
    );
    assert!(
        maya_signer::sealed::read_passphrase_file(&path).is_err(),
        "an Everyone-readable passphrase file was accepted"
    );

    // Owner-only, no inherited entries: accepted.
    icacls(&path, &["/inheritance:r", "/grant:r", &format!("{me}:F")]);
    icacls(&path, &["/remove", "*S-1-1-0"]);
    let read = maya_signer::sealed::read_passphrase_file(&path).unwrap();
    assert_eq!(read.as_slice(), b"correct horse battery");
}
