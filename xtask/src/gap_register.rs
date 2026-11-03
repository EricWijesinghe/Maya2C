//! `cargo xtask gap-register` — generates `reports/11-gap-register.md` (Master
//! Prompt 11 §1). Prints the report to stdout; redirect it to the file.
//!
//! Lists every TODO / FIXME, `todo!()`, `unimplemented!()`, `panic!()` outside
//! test code, `#[ignore]`d test, and mock/stub/fake on a production path, with
//! `file:line`, owning crate and a severity assigned by RULE, not by judgement:
//!
//!   P0 (blocks mainnet)  in a mainnet-core crate (ADR-016) and not test code
//!   P1 (blocks testnet)  in a service or client crate a testnet runs
//!   P2 (later)           everything else, and every #[ignore]
//!
//! A `panic!` counts only outside `#[cfg(test)]` modules and `tests/`, `benches/`,
//! `examples/` directories. Test-module detection is lexical: everything after
//! the first `#[cfg(test)]` in a file is test code, which is how this tree lays
//! out its unit tests. A module declared under `#[cfg(kani)]` is verification
//! code, compiled into no binary, and is treated the same way.
//!
//! A row whose line, or one of the three lines above it, carries a
//! `reviewed-guard:` comment is listed as "guard" instead of P0/P1: a reviewer
//! has written down why reaching it is a bug worth a panic (DECISIONS.md,
//! 2026-09-28). It stays in the register so the call can be disagreed with.

use std::fmt::Write as _;
use std::path::Path;
use std::process::{Command, ExitCode};

use regex::Regex;

const CORE: &[&str] = &[
    "crates/node",
    "crates/crypto-pq",
    "crates/vm",
    "crates/fee-market",
    "crates/governance",
    "crates/ledger-math",
    "crates/zk-stark",
    "crates/dag-bft",
    "bins/maya2c-node",
];
const TESTNET: &[&str] = &[
    "crates/api-gateway",
    "apps/explorer",
    "apps/faucet",
    "crates/telemetry",
    "bins/l1-wallet",
    "apps/wallet-gui/core",
    "crates/light-client",
    "crates/custody-mpc",
    "bins/maya2c-miner",
];
const SKIP_PREFIXES: &[&str] = &[
    "fuzz/",
    "offsec-sandbox/",
    "apps/wallet-gui/ui/target",
    "target",
    "attic/",
];
const GUARD_MARK: &str = "reviewed-guard:";
const GUARD_WINDOW: usize = 3;
const TEXT_WIDTH: usize = 110;

/// `(kind, pattern)` in report order.
const PATTERNS: &[(&str, &str)] = &[
    ("TODO/FIXME", r"//.*\b(TODO|FIXME|XXX)\b"),
    ("todo!()", r"\btodo!\s*\("),
    ("unimplemented!()", r"\bunimplemented!\s*\("),
    ("panic!()", r"\bpanic!\s*\("),
    ("#[ignore]", r"#\[ignore"),
    (
        "mock/stub on a non-test path",
        r"\b(Mock|mock_|Stub|stub_|Fake|fake_)[A-Za-z_]*\b",
    ),
];

const PANIC: &str = "panic!()";
const MOCK: &str = "mock/stub on a non-test path";
const IGNORE: &str = "#[ignore]";

struct Row {
    sev: &'static str,
    file: String,
    line: usize,
    crate_name: String,
    kind: &'static str,
    text: String,
}

pub fn run(_args: &[String]) -> ExitCode {
    match render(&crate::workspace_root()) {
        Ok(report) => {
            print!("{report}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("gap-register: {e}");
            ExitCode::FAILURE
        }
    }
}

fn render(root: &Path) -> Result<String, String> {
    let files = tracked_rust_files(root)?;
    let members = workspace_members(root)?;
    let proofs = kani_modules(root, &files)?;
    let mut rows = Vec::new();
    for file in &files {
        if SKIP_PREFIXES.iter().any(|p| file.starts_with(p)) {
            continue;
        }
        scan_file(root, file, &members, &proofs, &mut rows)?;
    }
    rows.sort_by(|a, b| (a.sev, &a.file, a.line).cmp(&(b.sev, &b.file, b.line)));
    Ok(report(&rows))
}

fn tracked_rust_files(root: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("git")
        .args(["ls-files", "*.rs"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !out.status.success() {
        return Err("git ls-files *.rs failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

/// Member paths from the root `Cargo.toml`'s `members` array, plus the three
/// crates that sit outside `crates/`, `bins/` and `apps/` by name.
fn workspace_members(root: &Path) -> Result<Vec<String>, String> {
    let text = read(&root.join("Cargo.toml"))?;
    let rx = Regex::new(r#"^\s*"([^"]+)","#).map_err(|e| e.to_string())?;
    Ok(text
        .lines()
        .filter_map(|line| rx.captures(line).map(|c| c[1].to_string()))
        .filter(|m| m.contains('/') || matches!(m.as_str(), "sim" | "econ" | "xtask"))
        .collect())
}

/// Files declared as `#[cfg(kani)] mod x;` in a `lib.rs` (proof code).
fn kani_modules(root: &Path, files: &[String]) -> Result<Vec<String>, String> {
    let rx =
        Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;").map_err(|e| e.to_string())?;
    let mut found = Vec::new();
    for f in files.iter().filter(|f| f.ends_with("/lib.rs")) {
        let lines = lines_of(&root.join(f))?;
        for pair in lines.windows(2) {
            if !pair[0].contains("#[cfg(kani)]") {
                continue;
            }
            if let Some(c) = rx.captures(&pair[1]) {
                found.push(format!("{}{}.rs", &f[..f.len() - "lib.rs".len()], &c[1]));
            }
        }
    }
    Ok(found)
}

fn scan_file(
    root: &Path,
    file: &str,
    members: &[String],
    proofs: &[String],
    rows: &mut Vec<Row>,
) -> Result<(), String> {
    let is_test_file = ["/tests/", "/benches/", "/examples/"]
        .iter()
        .any(|p| file.contains(p))
        || proofs.iter().any(|p| p == file);
    let lines = lines_of(&root.join(file))?;
    let patterns: Vec<(&str, Regex)> = PATTERNS
        .iter()
        .map(|(k, p)| Regex::new(p).map(|rx| (*k, rx)).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let mut in_test = is_test_file;
    for (idx, line) in lines.iter().enumerate() {
        let n = idx + 1;
        if line.contains("#[cfg(test)]") {
            in_test = true;
        }
        for (kind, rx) in &patterns {
            if !rx.is_match(line) {
                continue;
            }
            if (*kind == PANIC || *kind == MOCK) && in_test {
                continue;
            }
            if *kind == IGNORE && !in_test {
                continue;
            }
            let crate_name = owner(file, members);
            let mut sev = severity(kind, &crate_name, in_test);
            let window = &lines[(n - 1).saturating_sub(GUARD_WINDOW)..n];
            if (sev == "P0" || sev == "P1") && window.iter().any(|w| w.contains(GUARD_MARK)) {
                sev = "guard";
            }
            rows.push(Row {
                sev,
                file: file.to_string(),
                line: n,
                crate_name,
                kind,
                text: line.trim().chars().take(TEXT_WIDTH).collect(),
            });
        }
    }
    Ok(())
}

/// Longest workspace member that is a path prefix of `file`.
fn owner(file: &str, members: &[String]) -> String {
    let mut best = "";
    for m in members {
        if file.starts_with(&format!("{m}/")) && m.len() > best.len() {
            best = m;
        }
    }
    if best.is_empty() {
        "(outside the workspace)".to_string()
    } else {
        best.to_string()
    }
}

fn severity(kind: &str, crate_name: &str, in_test: bool) -> &'static str {
    if kind == IGNORE || in_test {
        "P2"
    } else if CORE.contains(&crate_name) {
        "P0"
    } else if TESTNET.contains(&crate_name) {
        "P1"
    } else {
        "P2"
    }
}

fn report(rows: &[Row]) -> String {
    let count = |s: &str| rows.iter().filter(|r| r.sev == s).count();
    let (p0, p1, p2, guard) = (count("P0"), count("P1"), count("P2"), count("guard"));
    let mut out = String::new();
    // Each `writeln!` is one Python `print`, so the blank lines match.
    let _ = writeln!(out, "# Gap register (Master Prompt 11 §1)\n");
    let _ = writeln!(
        out,
        "Generated by `cargo xtask gap-register` from the working tree; do not edit by hand."
    );
    let _ = writeln!(
        out,
        "Severity is assigned by the rule in the command's module docs, so a reviewer can"
    );
    let _ = writeln!(
        out,
        "disagree with the rule rather than with a thousand individual calls.\n"
    );
    let _ = writeln!(
        out,
        "**{} items: {p0} P0, {p1} P1, {p2} P2, {guard} reviewed guards.**\n",
        rows.len()
    );
    let _ = writeln!(
        out,
        "Caveats: test-module detection is lexical (after the first `#[cfg(test)]`), so a"
    );
    let _ = writeln!(
        out,
        "`panic!` in a test helper above that line is counted; a `panic!` that documents an"
    );
    let _ = writeln!(
        out,
        "unreachable invariant is still counted, because the register lists them for review,"
    );
    let _ = writeln!(
        out,
        "it does not judge them. A `guard` row carries a `reviewed-guard:` comment saying"
    );
    let _ = writeln!(
        out,
        "why reaching it is a bug; it is not a P0/P1 gap but stays listed for review.\n"
    );
    let _ = writeln!(out, "| Sev | Location | Crate | Kind | Line |");
    let _ = writeln!(out, "|---|---|---|---|---|");
    for r in rows {
        let text = r.text.replace('|', "\\|");
        let _ = writeln!(
            out,
            "| {} | `{}:{}` | `{}` | {} | `{text}` |",
            r.sev, r.file, r.line, r.crate_name, r.kind
        );
    }
    out
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read(path)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn lines_of(path: &Path) -> Result<Vec<String>, String> {
    Ok(read(path)?.lines().map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::{owner, severity};

    #[test]
    fn severity_follows_the_crate_tier() {
        assert_eq!(severity("panic!()", "crates/node", false), "P0");
        assert_eq!(severity("panic!()", "apps/explorer", false), "P1");
        assert_eq!(severity("panic!()", "crates/x", false), "P2");
        assert_eq!(severity("#[ignore]", "crates/node", false), "P2");
    }

    #[test]
    fn owner_is_the_longest_member_prefix() {
        let members = vec!["crates/node".to_string(), "crates/node/sub".to_string()];
        assert_eq!(owner("crates/node/sub/a.rs", &members), "crates/node/sub");
        assert_eq!(owner("tools/a.rs", &members), "(outside the workspace)");
    }
}
