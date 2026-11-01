//! `cargo xtask proof-ratchet`: the chain's immune memory may grow, never shrink.
//!
//! ADR-078 decision O3 (MIP-0002 §5). The evidence that a property holds,
//! whether a Kani harness, a conformance vector, an exploit replay or a fuzz
//! target, is counted. CI fails when any count falls below the committed
//! baseline in `proof-ratchet.txt`. A wrong test gets fixed rather than
//! deleted; a rename keeps the count. When a count legitimately rises, run
//! `--update` so the new floor is a decision somebody made.
//!
//! Counting is lexical, like `scripts/gap_register.py`. It counts what a
//! reviewer would count and reads no build output, so it runs before any
//! compile.
//!
//!   cargo xtask proof-ratchet            report the counts
//!   cargo xtask proof-ratchet --check    fail if any count is below the baseline
//!   cargo xtask proof-ratchet --update   write the current counts as the baseline

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const BASELINE: &str = "proof-ratchet.txt";
/// Directories never walked: build output, vendored or archived code.
const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "attic",
    "vendor",
    ".claude",
];
const KANI_MARK: &str = "#[kani::proof]";
const EXPLOIT_REPLAYS: &str = "crates/node/tests/exploit_replays.rs";

/// The counts, by name, in a stable order.
pub fn counts(root: &Path) -> Result<BTreeMap<&'static str, usize>, String> {
    let mut out = BTreeMap::new();
    out.insert("kani_harnesses", count_kani(root)?);
    out.insert(
        "conformance_vectors",
        count_spec_vectors(&root.join("spec/tests"))?,
    );
    out.insert(
        "acvp_vector_files",
        count_files(&root.join("crates/crypto-pq/tests/vectors/acvp"), "json"),
    );
    out.insert("exploit_replays", count_tests(&root.join(EXPLOIT_REPLAYS))?);
    out.insert(
        "fuzz_targets",
        count_files(&root.join("fuzz/fuzz_targets"), "rs"),
    );
    Ok(out)
}

/// `#[kani::proof]` lines in every tracked Rust source outside skipped trees.
fn count_kani(root: &Path) -> Result<usize, String> {
    let mut total = 0;
    for file in rust_files(root) {
        let text = std::fs::read_to_string(&file)
            .map_err(|e| format!("reading {}: {e}", file.display()))?;
        total += text
            .lines()
            .filter(|l| l.trim_start().starts_with(KANI_MARK))
            .count();
    }
    Ok(total)
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    let mut found = Vec::new();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if !SKIP_DIRS.iter().any(|s| name == *s) {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|x| x == "rs") {
                found.push(path);
            }
        }
    }
    found
}

/// Entries in the `cases` array of every `spec/tests/*.json`. A file without
/// `cases` (a constants table) contributes nothing.
fn count_spec_vectors(dir: &Path) -> Result<usize, String> {
    let mut total = 0;
    let entries = std::fs::read_dir(dir).map_err(|e| format!("reading {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("reading {}: {e}", path.display()))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("{} does not parse: {e}", path.display()))?;
        total += value
            .get("cases")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
    }
    Ok(total)
}

/// Files with extension `ext` directly in `dir` (0 if the directory is absent).
fn count_files(dir: &Path, ext: &str) -> usize {
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .filter(|e| e.path().is_file() && e.path().extension().is_some_and(|x| x == ext))
            .count()
    })
}

/// `#[test]` attributes in one file.
fn count_tests(file: &Path) -> Result<usize, String> {
    let text =
        std::fs::read_to_string(file).map_err(|e| format!("reading {}: {e}", file.display()))?;
    Ok(text
        .lines()
        .filter(|l| l.trim_start().starts_with("#[test]"))
        .count())
}

/// Parses `name = count` lines; `#` starts a comment.
fn parse_baseline(text: &str) -> Result<BTreeMap<String, usize>, String> {
    let mut out = BTreeMap::new();
    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| format!("{BASELINE}:{}: expected `name = count`", n + 1))?;
        let value: usize = value
            .trim()
            .parse()
            .map_err(|e| format!("{BASELINE}:{}: {e}", n + 1))?;
        out.insert(name.trim().to_string(), value);
    }
    Ok(out)
}

/// Every count that fell below its baseline, as a message. A name in the
/// baseline that is no longer counted is also a failure: deleting a counter
/// must not be a way around the ratchet.
fn regressions(now: &BTreeMap<&'static str, usize>, base: &BTreeMap<String, usize>) -> Vec<String> {
    let mut out = Vec::new();
    for (name, floor) in base {
        match now.get(name.as_str()) {
            Some(count) if count < floor => {
                out.push(format!("{name}: {count} < baseline {floor}"));
            }
            Some(_) => {}
            None => out.push(format!("{name}: in the baseline but no longer counted")),
        }
    }
    out
}

fn render(now: &BTreeMap<&'static str, usize>) -> String {
    let mut s = String::from(
        "# Proof ratchet baseline (ADR-078 O3). Counts may rise freely and never fall.\n\
         # Raise with `cargo xtask proof-ratchet --update` and say why in the commit.\n",
    );
    for (name, count) in now {
        s.push_str(&format!("{name} = {count}\n"));
    }
    s
}

pub fn run(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let now = counts(&root)?;
    for (name, count) in &now {
        println!("{name:<22} {count}");
    }
    match args.first().map(String::as_str) {
        None | Some("--report") => Ok(()),
        Some("--update") => {
            std::fs::write(root.join(BASELINE), render(&now))
                .map_err(|e| format!("writing {BASELINE}: {e}"))?;
            println!("proof-ratchet: baseline written to {BASELINE}");
            Ok(())
        }
        Some("--check") => {
            let text = std::fs::read_to_string(root.join(BASELINE))
                .map_err(|e| format!("reading {BASELINE}: {e}"))?;
            let failures = regressions(&now, &parse_baseline(&text)?);
            if failures.is_empty() {
                println!("proof-ratchet: no count below its baseline");
                Ok(())
            } else {
                Err(format!(
                    "proof-ratchet: evidence was removed (ADR-078 O3):\n  {}",
                    failures.join("\n  ")
                ))
            }
        }
        Some(other) => Err(format!("proof-ratchet: unknown argument {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now(pairs: &[(&'static str, usize)]) -> BTreeMap<&'static str, usize> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn a_count_below_its_baseline_fails_and_a_rise_passes() {
        let base = parse_baseline("kani_harnesses = 80\nfuzz_targets = 16 # floor\n").unwrap();
        assert!(
            regressions(&now(&[("kani_harnesses", 81), ("fuzz_targets", 16)]), &base).is_empty()
        );
        let fell = regressions(&now(&[("kani_harnesses", 79), ("fuzz_targets", 16)]), &base);
        assert_eq!(fell, vec!["kani_harnesses: 79 < baseline 80".to_string()]);
    }

    #[test]
    fn dropping_a_counter_is_not_a_way_around_the_ratchet() {
        let base = parse_baseline("kani_harnesses = 80\n").unwrap();
        assert_eq!(regressions(&now(&[]), &base).len(), 1);
    }

    #[test]
    fn a_malformed_baseline_line_is_an_error() {
        assert!(parse_baseline("kani_harnesses 80\n").is_err());
        assert!(parse_baseline("kani_harnesses = many\n").is_err());
    }

    #[test]
    fn the_baseline_round_trips_through_render() {
        let counts = now(&[("a", 1), ("b", 2)]);
        let parsed = parse_baseline(&render(&counts)).unwrap();
        assert_eq!(parsed.get("a"), Some(&1));
        assert_eq!(parsed.get("b"), Some(&2));
    }
}
