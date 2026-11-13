//! `maya2c-genesis`, driven through its command line: mint, then verify.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const A: &str = "11111111111111111111111111111111111111111111111111111111111111aa";
const B: &str = "22222222222222222222222222222222222222222222222222222222222222bb";

/// A fresh directory under the OS temp dir, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("maya2c-genesis-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_maya2c-genesis"))
        .args(args)
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Refused with exit 1, never a panic (101).
fn refused(out: &Output) -> String {
    assert_eq!(out.status.code(), Some(1), "{}", text(out));
    text(out)
}

fn mint(out: &Path, allocs: &[&str]) -> Output {
    let mut args = vec![
        "--chain-id",
        "test-genesis-cli",
        "--timestamp",
        "1756252800",
        "--difficulty-bits",
        "4",
        "--out",
        out.to_str().unwrap(),
    ];
    for a in allocs {
        args.extend(["--alloc", a]);
    }
    run(&args)
}

fn state_root(out: &Output) -> String {
    text(out)
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("state root:")
                .map(|r| r.trim().to_owned())
        })
        .expect("mint prints the state root")
}

#[test]
fn verify_passes_on_the_minted_root_and_fails_on_any_other() {
    let dir = Scratch::new("verify");
    let file = dir.0.join("genesis.json");
    let minted = mint(&file, &[&format!("{A}:1000"), &format!("0x{B}:5")]);
    assert!(minted.status.success(), "{}", text(&minted));
    let root = state_root(&minted);
    assert_eq!(root.len(), 64);
    let f = file.to_str().unwrap();

    let ok = run(&[
        "--verify",
        f,
        "--expect-state-root",
        &root.to_ascii_uppercase(),
    ]);
    assert!(ok.status.success(), "{}", text(&ok));
    assert!(text(&ok).contains("matches"));
    // Decimal base units: `:1000` once parsed as hex, 4096.
    assert!(text(&ok).contains("total supply   1005"), "{}", text(&ok));

    let mut wrong = root.into_bytes();
    wrong[0] = if wrong[0] == b'0' { b'1' } else { b'0' };
    let wrong = String::from_utf8(wrong).unwrap();
    let err = refused(&run(&["--verify", f, "--expect-state-root", &wrong]));
    assert!(err.contains("state root mismatch"), "{err}");

    // A file edited after the ceremony no longer produces the published root.
    let json = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, json.replace("1000", "1001")).unwrap();
    let err = refused(&run(&[
        "--verify",
        f,
        "--expect-state-root",
        &state_root(&minted),
    ]));
    assert!(err.contains("state root mismatch"), "{err}");
}

#[test]
fn duplicate_allocations_are_refused_at_mint_and_at_verify() {
    let dir = Scratch::new("dup");
    let file = dir.0.join("genesis.json");
    // The second spelling normalises to the first address.
    let err = refused(&mint(
        &file,
        &[
            &format!("{A}:1"),
            &format!("0x{}:2", A.to_ascii_uppercase()),
        ],
    ));
    assert!(err.contains("duplicate allocation"), "{err}");
    assert!(!file.exists(), "a refused genesis was written");

    // A hand-edited file that repeats an address fails verification.
    assert!(
        mint(&file, &[&format!("{A}:1"), &format!("{B}:2")])
            .status
            .success()
    );
    let json = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, json.replace(B, &A.to_ascii_uppercase())).unwrap();
    let err = refused(&run(&["--verify", file.to_str().unwrap()]));
    assert!(err.contains("duplicate allocation"), "{err}");
}

#[test]
fn malformed_allocations_and_files_are_errors_not_panics() {
    let dir = Scratch::new("bad");
    let file = dir.0.join("genesis.json");
    for bad in [
        "no-colon".to_owned(),
        format!("{A}:"),
        format!("{A}:-1"),
        format!("{A}:1e3"),
        format!("zz{}:1", &A[2..]),
        format!("{}:1", &A[..62]),
        format!("{A}:1{}", "0".repeat(78)),
        format!("é{}:1", &A[2..]),
    ] {
        refused(&mint(&file, &[&bad]));
    }
    assert!(!file.exists());

    std::fs::write(&file, "{ not json").unwrap();
    refused(&run(&["--verify", file.to_str().unwrap()]));
    refused(&run(&[
        "--verify",
        dir.0.join("absent.json").to_str().unwrap(),
    ]));
}

#[test]
fn an_existing_genesis_is_not_overwritten_without_force() {
    let dir = Scratch::new("force");
    let file = dir.0.join("genesis.json");
    assert!(mint(&file, &[&format!("{A}:1")]).status.success());
    let before = std::fs::read(&file).unwrap();
    let err = refused(&mint(&file, &[&format!("{B}:9")]));
    assert!(err.contains("already exists"), "{err}");
    assert_eq!(std::fs::read(&file).unwrap(), before);
}
