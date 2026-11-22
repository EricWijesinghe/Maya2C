//! `cargo xtask verify-release <dir> [--checksums-only]`: checks a downloaded
//! release before anyone trusts it (docs/verify-release.md).
//!
//! Three independent checks, in order:
//!
//! 1. `SHA256SUMS` matches every file in the directory, and no file is
//!    unlisted. Computed here, so it needs nothing installed.
//! 2. `cosign verify-blob` on each file and on `SHA256SUMS`, against the
//!    Sigstore bundle (`<name>.sigstore.json`) the release workflow uploaded.
//!    The certificate must come from the release workflow on a `node-v*` tag.
//! 3. `gh attestation verify` on each file and on `SHA256SUMS`: GitHub's SLSA
//!    build provenance, one attestation per subject.
//!
//! Steps 2 and 3 shell out to `cosign` and `gh`, the reference verifiers.
//! Re-implementing Sigstore here would be a second verifier to trust.
//! `--checksums-only` skips them and says so.

use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;
use std::process::Command;

const REPO: &str = "EricWijesinghe/Maya2C";
const OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";
/// Only the release workflow, run on a `node-v*` tag, may sign a release.
const IDENTITY_REGEX: &str = r"^https://github\.com/EricWijesinghe/Maya2C/\.github/workflows/release-binaries\.yml@refs/tags/node-v.+$";
const SUMS: &str = "SHA256SUMS";
const BUNDLE_SUFFIX: &str = ".sigstore.json";
const HASH_HEX_LEN: usize = 64;
const SEPARATOR: &str = "  "; // sha256sum text mode

pub fn run(args: &[String]) -> Result<(), String> {
    let (dir, checksums_only) = parse_args(args)?;
    let entries = verify_checksums(&dir)?;
    println!("checksums: {} files match {SUMS}", entries.len());
    if checksums_only {
        println!("signatures: NOT CHECKED (--checksums-only)");
        return Ok(());
    }
    let mut targets: Vec<&str> = entries.iter().map(|(_, n)| n.as_str()).collect();
    targets.push(SUMS);
    for name in &targets {
        cosign_verify(&dir.join(name), &dir.join(format!("{name}{BUNDLE_SUFFIX}")))?;
        println!("cosign: {name} ok");
    }
    for name in &targets {
        gh_attest_verify(&dir.join(name))?;
        println!("attestation: {name} ok");
    }
    println!("verify-release: all checks passed");
    Ok(())
}

/// SHA256SUMS against the directory: every listed file hashes to its listed
/// value, and no file is unlisted. Returns the listed entries.
fn verify_checksums(dir: &Path) -> Result<Vec<(String, String)>, String> {
    let sums_path = dir.join(SUMS);
    let text =
        fs::read_to_string(&sums_path).map_err(|e| format!("{}: {e}", sums_path.display()))?;
    let entries = parse_sums(&text)?;
    check_unlisted(dir, &entries)?;
    for (listed, name) in &entries {
        let actual = sha256_file(&dir.join(name))?;
        if &actual != listed {
            return Err(format!(
                "{name}: checksum mismatch (listed {listed}, actual {actual})"
            ));
        }
    }
    Ok(entries)
}

fn parse_args(args: &[String]) -> Result<(std::path::PathBuf, bool), String> {
    let usage = "usage: cargo xtask verify-release <dir> [--checksums-only]";
    let mut dir = None;
    let mut checksums_only = false;
    for a in args {
        match a.as_str() {
            "--checksums-only" => checksums_only = true,
            s if s.starts_with("--") => return Err(format!("unknown flag {s}\n{usage}")),
            s if dir.is_none() => dir = Some(std::path::PathBuf::from(s)),
            _ => return Err(usage.to_string()),
        }
    }
    dir.map(|d| (d, checksums_only))
        .ok_or_else(|| usage.to_string())
}

/// Parses `sha256sum` output: `<64 hex>  <name>`, one per line. Names are
/// bare file names; anything with a path separator or `..` is refused, so a
/// hostile SHA256SUMS cannot point the check at a file outside the directory.
fn parse_sums(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let hash = line
            .get(..HASH_HEX_LEN)
            .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| format!("not a sha256sum line: {line}"))?;
        // `  ` is text mode (Linux CI); ` *` is binary mode (Windows sha256sum).
        let rest = &line[HASH_HEX_LEN..];
        let name = rest
            .strip_prefix(SEPARATOR)
            .or_else(|| rest.strip_prefix(" *"))
            .ok_or_else(|| format!("not a sha256sum line: {line}"))?;
        let bare = !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains(['/', '\\'])
            && name != SUMS;
        if !bare {
            return Err(format!("refusing unsafe file name in {SUMS}: {name:?}"));
        }
        out.push((hash.to_ascii_lowercase(), name.to_string()));
    }
    if out.is_empty() {
        return Err(format!("{SUMS} lists no files"));
    }
    Ok(out)
}

/// Every file in the directory must be listed, or be the sums file itself or
/// a signature bundle. An unlisted file was not covered by the signature.
fn check_unlisted(dir: &Path, entries: &[(String, String)]) -> Result<(), String> {
    for item in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let item = item.map_err(|e| e.to_string())?;
        let name = item.file_name().to_string_lossy().into_owned();
        if name == SUMS || name.ends_with(BUNDLE_SUFFIX) {
            continue;
        }
        if !item.file_type().map_err(|e| e.to_string())?.is_file() {
            return Err(format!("{name}: not a plain file in the release directory"));
        }
        if !entries.iter().any(|(_, n)| *n == name) {
            return Err(format!("{name}: not listed in {SUMS}"));
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    sha256_reader(&mut f).map_err(|e| format!("{}: {e}", path.display()))
}

fn sha256_reader<R: Read>(r: &mut R) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn cosign_verify(file: &Path, bundle: &Path) -> Result<(), String> {
    let status = Command::new("cosign")
        .args(["verify-blob", "--bundle"])
        .arg(bundle)
        .args(["--certificate-identity-regexp", IDENTITY_REGEX])
        .args(["--certificate-oidc-issuer", OIDC_ISSUER])
        .arg(file)
        .status()
        .map_err(|e| tool_error("cosign", &e))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cosign rejected {}", file.display()))
    }
}

fn gh_attest_verify(file: &Path) -> Result<(), String> {
    let status = Command::new("gh")
        .args(["attestation", "verify"])
        .arg(file)
        .args(["--repo", REPO])
        .status()
        .map_err(|e| tool_error("gh", &e))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("no valid build attestation for {}", file.display()))
    }
}

fn tool_error(tool: &str, e: &io::Error) -> String {
    format!("cannot run `{tool}` ({e}); install it, or use --checksums-only")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC_SHA256: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn sha256_matches_known_vector() {
        let got = sha256_reader(&mut &b"abc"[..]).expect("in-memory read");
        assert_eq!(got, ABC_SHA256);
    }

    #[test]
    fn parses_sha256sum_lines() {
        let text = format!("{ABC_SHA256}  maya2c-node.tar.gz\n");
        let got = parse_sums(&text).expect("valid line");
        assert_eq!(
            got,
            vec![(ABC_SHA256.to_string(), "maya2c-node.tar.gz".to_string())]
        );
        let binary = format!(
            "{ABC_SHA256} *maya2c-node.tar.gz
"
        );
        assert_eq!(
            parse_sums(&binary).expect("binary-mode line"),
            parse_sums(&text).expect("again")
        );
    }

    /// A release directory holding one file, listed in SHA256SUMS as sha256sum writes it.
    fn release_with(tag: &str, content: &[u8]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("verify-release-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");
        fs::write(dir.join("app.bin"), content).expect("write release file");
        let sums = format!("{}  app.bin\n", hex::encode(Sha256::digest(content)));
        fs::write(dir.join(SUMS), sums).expect("write SHA256SUMS");
        dir
    }

    #[test]
    fn untouched_release_passes_checksums() {
        let dir = release_with("untouched", b"abc");
        let entries = verify_checksums(&dir).expect("untouched release verifies");
        assert_eq!(
            entries,
            vec![(ABC_SHA256.to_string(), "app.bin".to_string())]
        );
    }

    #[test]
    fn tampered_file_fails_checksums() {
        let dir = release_with("tampered", b"abc");
        fs::write(dir.join("app.bin"), b"abd").expect("tamper");
        let err = verify_checksums(&dir).expect_err("a changed byte must fail");
        assert!(err.contains("checksum mismatch"), "{err}");
    }

    #[test]
    fn unlisted_file_fails_checksums() {
        let dir = release_with("unlisted", b"abc");
        fs::write(dir.join("extra.bin"), b"x").expect("extra file");
        let err = verify_checksums(&dir).expect_err("an unlisted file must fail");
        assert!(err.contains("not listed"), "{err}");
    }

    /// The signer identity the cosign check pins (wrong signer = wrong key).
    /// The cryptographic check itself runs only in the real tag run: NOT VERIFIED here.
    #[test]
    fn signer_identity_pins_release_workflow_on_a_node_tag() {
        let re = regex::Regex::new(IDENTITY_REGEX).expect("pattern compiles");
        let good = "https://github.com/EricWijesinghe/Maya2C/.github/workflows/release-binaries.yml@refs/tags/node-v0.1.0-testnet.1";
        assert!(re.is_match(good), "release identity rejected");
        for wrong in [
            "https://github.com/EricWijesinghe/Maya2C/.github/workflows/ci.yml@refs/tags/node-v0.1.0",
            "https://github.com/EricWijesinghe/Maya2C/.github/workflows/release-binaries.yml@refs/heads/master",
            "https://github.com/Someone/Maya2C/.github/workflows/release-binaries.yml@refs/tags/node-v0.1.0",
            "https://github.com/EricWijesinghe/Maya2C/.github/workflows/release-binaries.yml@refs/tags/wallet-v1",
        ] {
            assert!(!re.is_match(wrong), "accepted {wrong}");
        }
    }

    #[test]
    fn refuses_path_traversal_and_bad_lines() {
        for bad in [
            format!("{ABC_SHA256}  ../evil"),
            format!("{ABC_SHA256}  sub/evil"),
            format!("{ABC_SHA256}  SHA256SUMS"),
            format!("{ABC_SHA256} x"),
            "zz  short".to_string(),
            String::new(),
        ] {
            assert!(parse_sums(&bad).is_err(), "accepted: {bad:?}");
        }
    }
}
