//! `cargo xtask reality-audit <cargo-test-log> [--markdown]` — re-derives every
//! `features.toml` status from a test log (Master Prompt 11 §1).
//!
//! An entry keeps `working`/`verified` only if every test it names ran in the
//! given log and its binary reported `0 failed`. A test that did not run at all
//! counts as evidence missing, not as a pass. Entries backed by a `gate` (a
//! command, not a test) are reported as NEEDS-GATE and keep their status,
//! because a test log cannot say anything about them.

use std::collections::HashMap;
use std::path::Path;
use std::process::ExitCode;

use regex::Regex;
use serde::Deserialize;

const USAGE: &str = "usage: cargo xtask reality-audit <cargo-test-log> [--markdown]";
const GATE_PREVIEW: usize = 60;

#[derive(Deserialize)]
struct Ledger {
    #[serde(default)]
    subsystem: Vec<Entry>,
    #[serde(default)]
    feature: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
    status: String,
    #[serde(default)]
    tests: Vec<String>,
    #[serde(default)]
    gate: String,
}

/// `(passed, failed)` for one test binary.
type Counts = (u64, u64);

/// `(id, before, after, evidence)`.
type Row = (String, String, String, String);

pub fn run(args: &[String]) -> ExitCode {
    let Some(log_path) = args.first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match audit(log_path, args.iter().any(|a| a == "--markdown")) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("reality-audit: {e}");
            ExitCode::FAILURE
        }
    }
}

fn audit(log_path: &str, markdown: bool) -> Result<(), String> {
    let log = std::fs::read(log_path).map_err(|e| format!("{log_path}: {e}"))?;
    let runs = ran_binaries(&String::from_utf8_lossy(&log))?;
    let root = crate::workspace_root();
    let features = root.join("features.toml");
    let text =
        std::fs::read_to_string(&features).map_err(|e| format!("{}: {e}", features.display()))?;
    let ledger: Ledger = toml::from_str(&text).map_err(|e| format!("features.toml: {e}"))?;

    let mut rows: Vec<Row> = Vec::new();
    let mut changes = 0;
    for entry in ledger.subsystem.iter().chain(&ledger.feature) {
        if entry.status != "working" && entry.status != "verified" {
            continue;
        }
        let row = judge(entry, &runs);
        if row.1 != row.2 {
            changes += 1;
        }
        rows.push(row);
    }

    if markdown {
        println!("| id | before | after | evidence |");
        println!("|---|---|---|---|");
    }
    for (id, before, after, why) in &rows {
        if markdown {
            let shown = if before == after {
                after.clone()
            } else {
                format!("**{after}**")
            };
            println!("| `{id}` | {before} | {shown} | {why} |");
        } else if before != after || why.contains("NEEDS") {
            println!("{id:<40} {before:<9} -> {after:<9} {why}");
        }
    }
    println!(
        "\n{} claims examined, {changes} would change status on this log's evidence.",
        rows.len()
    );
    Ok(())
}

fn judge(entry: &Entry, runs: &HashMap<String, Counts>) -> Row {
    let before = entry.status.clone();
    if entry.tests.is_empty() {
        let gate: String = entry.gate.chars().take(GATE_PREVIEW).collect();
        return (
            entry.id.clone(),
            before.clone(),
            before,
            format!("NEEDS-GATE: {gate}"),
        );
    }
    let mut tests: Vec<&str> = Vec::new();
    for t in &entry.tests {
        if !tests.contains(&t.as_str()) {
            tests.push(t);
        }
    }
    let verdicts: Vec<(&str, &str)> = tests.iter().map(|t| (*t, check(t, runs))).collect();
    let named = |want: &str| -> String {
        verdicts
            .iter()
            .filter(|(_, v)| *v == want)
            .map(|(t, _)| *t)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (after, why) = if verdicts.iter().all(|(_, v)| *v == "pass") {
        (before.clone(), "all named tests ran and passed".to_string())
    } else if verdicts.iter().any(|(_, v)| *v == "fail") {
        (
            "stub".to_string(),
            format!("a named test FAILED: {}", named("fail")),
        )
    } else {
        (
            "stub".to_string(),
            format!("not in this run: {}", named("not-run")),
        )
    };
    (entry.id.clone(), before, after, why)
}

/// `pass`, `fail`, or `not-run` for one test named in `features.toml`.
fn check(test: &str, runs: &HashMap<String, Counts>) -> &'static str {
    let key = match test.strip_prefix("crate:") {
        Some(krate) => krate.replace('-', "_"),
        // `crates/x/tests/foo.rs` appears in the log as `tests/foo.rs`, and
        // several crates can have one; the stem names the binary uniquely.
        None => Path::new(test)
            .file_stem()
            .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
    };
    match runs.get(&key) {
        None => "not-run",
        Some((_, failed)) if *failed > 0 => "fail",
        Some(_) => "pass",
    }
}

/// Maps a test source path or crate name to `(passed, failed)`. A binary whose
/// source and dep names are the same is counted twice, as the script did.
fn ran_binaries(log: &str) -> Result<HashMap<String, Counts>, String> {
    let running = Regex::new(r"Running (.+?) \(target/[^/]+/deps/([a-z0-9_]+)-[0-9a-f]+\)")
        .map_err(|e| e.to_string())?;
    let result =
        Regex::new(r"^test result: \w+\. (\d+) passed; (\d+) failed").map_err(|e| e.to_string())?;
    let mut results: HashMap<String, Counts> = HashMap::new();
    let mut current: Option<(String, String)> = None;
    for line in log.lines() {
        if let Some(c) = running.captures(line) {
            current = Some((c[1].to_string(), c[2].to_string()));
            continue;
        }
        let Some(c) = result.captures(line) else {
            continue;
        };
        let (Some((src, dep)), Ok(passed), Ok(failed)) =
            (current.take(), c[1].parse::<u64>(), c[2].parse::<u64>())
        else {
            continue;
        };
        for key in [src, dep] {
            let entry = results.entry(key).or_insert((0, 0));
            entry.0 += passed;
            entry.1 += failed;
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::{check, ran_binaries};

    const LOG: &str = "     Running tests/fees.rs (target/debug/deps/fees-0a1b2c3d4e5f6a7b)\n\
test result: ok. 3 passed; 0 failed; 0 ignored\n";

    #[test]
    fn a_passing_binary_is_found_by_its_stem() {
        let runs = ran_binaries(LOG).expect("regex compiles");
        assert_eq!(check("crates/fee-market/tests/fees.rs", &runs), "pass");
        assert_eq!(check("crates/other/tests/absent.rs", &runs), "not-run");
    }

    #[test]
    fn a_failing_binary_is_a_fail() {
        let log = LOG.replace("0 failed", "1 failed");
        let runs = ran_binaries(&log).expect("regex compiles");
        assert_eq!(check("tests/fees.rs", &runs), "fail");
    }
}
