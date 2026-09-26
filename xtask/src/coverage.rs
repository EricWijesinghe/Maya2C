//! The reality-ledger gate.
//!
//! `features.toml` says what each subsystem is and whether it works. Nothing
//! stops somebody writing `status = "verified"` next to code that has never
//! been run — except this, which insists that the claim name a test that
//! exists, and fails CI when it does not.
//!
//! What it does **not** do is run the tests. A green run of this command means
//! every claim points at something real, not that the something passes. The
//! test job is what says that, and it is a separate job on purpose.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

const TIERS: [&str; 3] = ["core", "extended", "frontier"];
const CLASSES: [&str; 3] = ["REAL", "SIM", "RESEARCH"];
const STATUSES: [&str; 4] = ["planned", "stub", "working", "verified"];
/// A status in this set is a claim that something works, so it must be backed.
const CLAIMS_TO_WORK: [&str; 2] = ["working", "verified"];

#[derive(Deserialize)]
struct Ledger {
    /// The register the foundation brief asked for: prompts 1-162 plus
    /// Benchmark and Memory. It says what the *plan* is.
    #[serde(default)]
    feature: Vec<Feature>,
    /// The reality ledger: one entry per subsystem that exists, with the
    /// tests that would fail if it broke. It says what the *tree* is.
    #[serde(default)]
    subsystem: Vec<Feature>,
}

impl Ledger {
    /// Both tables. They share a schema and the same checks apply to each.
    fn all(&self) -> impl Iterator<Item = &Feature> {
        self.feature.iter().chain(self.subsystem.iter())
    }
}

#[derive(Deserialize)]
struct Feature {
    id: String,
    name: String,
    tier: String,
    class: String,
    status: String,
    #[serde(default)]
    tests: Vec<String>,
    #[serde(default)]
    gate: String,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let quiet = args.iter().any(|a| a == "--quiet");
    let verify = args.iter().any(|a| a == "--verify-targets");
    let root = crate::workspace_root();

    let path = root.join("features.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let ledger: Ledger =
        toml::from_str(&text).map_err(|e| format!("features.toml is not valid: {e}"))?;

    let packages = workspace_packages(&root)?;
    let mut problems: Vec<String> = Vec::new();
    let mut seen: BTreeMap<&str, ()> = BTreeMap::new();

    for f in ledger.all() {
        if seen.insert(&f.id, ()).is_some() {
            problems.push(format!("{}: duplicate id", f.id));
        }
        if !TIERS.contains(&f.tier.as_str()) {
            problems.push(format!(
                "{}: tier `{}` is not one of {TIERS:?}",
                f.id, f.tier
            ));
        }
        if !CLASSES.contains(&f.class.as_str()) {
            problems.push(format!(
                "{}: class `{}` is not one of {CLASSES:?}",
                f.id, f.class
            ));
        }
        if !STATUSES.contains(&f.status.as_str()) {
            problems.push(format!(
                "{}: status `{}` is not one of {STATUSES:?}",
                f.id, f.status
            ));
        }

        for t in &f.tests {
            if let Some(pkg) = t.strip_prefix("crate:") {
                if !packages.iter().any(|p| p == pkg) {
                    problems.push(format!("{}: `{t}` names no workspace package", f.id));
                }
            } else if !root.join(t).exists() {
                problems.push(format!("{}: test `{t}` does not exist", f.id));
            }
        }

        if CLAIMS_TO_WORK.contains(&f.status.as_str()) && f.tests.is_empty() && f.gate.is_empty() {
            problems.push(format!(
                "{}: status `{}` but no test and no gate — a claim nothing checks",
                f.id, f.status
            ));
        }
        if f.status == "planned" && !f.tests.is_empty() {
            problems.push(format!(
                "{}: status `planned` but names tests; planned means no code",
                f.id
            ));
        }
    }

    if !quiet {
        println!(
            "
== register: the plan =="
        );
        print_table(&ledger.feature);
        println!(
            "
== ledger: the tree =="
        );
        print_table(&ledger.subsystem);
    }

    if verify && problems.is_empty() {
        problems.extend(verify_targets(&ledger)?);
    }

    if problems.is_empty() {
        println!(
            "
features.toml: {} register entries + {} subsystems, all claims backed{}.",
            ledger.feature.len(),
            ledger.subsystem.len(),
            if verify {
                " and every named test is a target cargo-nextest can run"
            } else {
                ""
            }
        );
        Ok(())
    } else {
        for p in &problems {
            eprintln!("  {p}");
        }
        Err(format!(
            "{} problem(s) in features.toml — see above",
            problems.len()
        ))
    }
}

/// Ask cargo-nextest what tests actually exist, and check the ledger against
/// it.
///
/// `run` on its own checks that a named test *file* is on disk. That is not
/// the same claim: a file can exist and compile into no runnable target — an
/// integration test excluded by `[[test]] test = false`, or a `crate:` entry
/// naming a package whose `#[cfg(test)]` modules were all deleted. This asks
/// the runner.
///
/// It does not run them. A green `cargo nextest run --workspace` in the same
/// job is what says they pass; this says the ledger's names point at things
/// that job would have executed.
fn verify_targets(ledger: &Ledger) -> Result<Vec<String>, String> {
    // Format and colour pinned: CI sets CARGO_TERM_COLOR=always, and ANSI
    // escapes around `pkg::binary` made every entry look missing.
    let out = std::process::Command::new("cargo")
        .args([
            "nextest",
            "list",
            "--workspace",
            "--run-ignored",
            "all",
            "--message-format",
            "oneline",
            "--color",
            "never",
        ])
        .output()
        .map_err(|e| format!("cannot run `cargo nextest list`: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "`cargo nextest list --workspace --run-ignored all` failed:
{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);

    // Lines are `<package> <test>` for a unit test and
    // `<package>::<binary> <test>` for an integration test.
    let mut packages_with_tests: BTreeMap<&str, ()> = BTreeMap::new();
    let mut binaries: BTreeMap<&str, ()> = BTreeMap::new();
    for line in text.lines() {
        let Some(target) = line.split_whitespace().next() else {
            continue;
        };
        match target.split_once("::") {
            Some((pkg, bin)) => {
                packages_with_tests.insert(pkg, ());
                binaries.insert(bin, ());
            }
            None => {
                packages_with_tests.insert(target, ());
            }
        }
    }

    let mut problems = Vec::new();
    for f in ledger.all() {
        if !CLAIMS_TO_WORK.contains(&f.status.as_str()) {
            continue;
        }
        for t in &f.tests {
            if let Some(pkg) = t.strip_prefix("crate:") {
                if !packages_with_tests.contains_key(pkg) {
                    problems.push(format!("{}: `{t}` has no test cargo-nextest can run", f.id));
                }
            } else {
                // `a/b/tests/name.rs` compiles to a binary called `name`.
                let stem = t.rsplit('/').next().unwrap_or(t).trim_end_matches(".rs");
                // A `gate` is the entry saying "this one is not run by the
                // default workspace test job, and here is what does run it" —
                // a platform-gated target, or a crate outside the workspace.
                if !binaries.contains_key(stem) && f.gate.is_empty() {
                    problems.push(format!(
                        "{}: `{t}` is not a target the workspace test job runs, and the entry names no `gate` that does",
                        f.id
                    ));
                }
            }
        }
    }
    Ok(problems)
}

fn print_table(features: &[Feature]) {
    println!(
        "{:<48} {:<9} {:<9} {:<9} {:>5}",
        "id", "tier", "class", "status", "tests"
    );
    println!("{:-<48} {:-<9} {:-<9} {:-<9} {:->5}", "", "", "", "", "");
    for f in features {
        println!(
            "{:<48} {:<9} {:<9} {:<9} {:>5}",
            truncate(&f.id, 48),
            f.tier,
            f.class,
            f.status,
            f.tests.len()
        );
    }

    let mut by_status: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_tier: BTreeMap<&str, usize> = BTreeMap::new();
    let mut by_class: BTreeMap<&str, usize> = BTreeMap::new();
    for f in features {
        *by_status.entry(&f.status).or_default() += 1;
        *by_tier.entry(&f.tier).or_default() += 1;
        *by_class.entry(&f.class).or_default() += 1;
    }
    println!("\nstatus  {by_status:?}");
    println!("tier    {by_tier:?}");
    println!("class   {by_class:?}");
    let unnamed = features.iter().filter(|f| f.name.is_empty()).count();
    if unnamed > 0 {
        println!("{unnamed} entries have no name");
    }
}

fn truncate(s: &str, n: usize) -> &str {
    if s.len() <= n { s } else { &s[..n] }
}

/// Workspace package names, read from the manifests rather than from
/// `cargo metadata` — this command must run before anything is built.
fn workspace_packages(root: &Path) -> Result<Vec<String>, String> {
    #[derive(Deserialize)]
    struct Root {
        workspace: Workspace,
        package: Option<Pkg>,
    }
    #[derive(Deserialize)]
    struct Workspace {
        members: Vec<String>,
    }
    #[derive(Deserialize)]
    struct Pkg {
        name: String,
    }

    let text = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|e| format!("cannot read the root manifest: {e}"))?;
    let manifest: Root =
        toml::from_str(&text).map_err(|e| format!("the root manifest is not valid: {e}"))?;

    let mut names: Vec<String> = Vec::new();
    if let Some(p) = manifest.package {
        names.push(p.name);
    }
    for member in manifest.workspace.members {
        let path = root.join(&member).join("Cargo.toml");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let m: Root2 =
            toml::from_str(&text).map_err(|e| format!("{} is not valid: {e}", path.display()))?;
        names.push(m.package.name);
    }
    Ok(names)
}

#[derive(Deserialize)]
struct Root2 {
    package: Pkg2,
}
#[derive(Deserialize)]
struct Pkg2 {
    name: String,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_committed_ledger_parses_and_every_claim_is_backed() {
        // The gate run against the real file. If this fails, `features.toml`
        // claims something it cannot show.
        run(&["--quiet".to_string()]).expect("features.toml gate");
    }

    #[test]
    fn workspace_packages_includes_the_root_and_the_members() {
        let names = workspace_packages(&crate::workspace_root()).expect("manifests");
        assert!(names.contains(&"custom-l1-node".to_string()));
        assert!(names.contains(&"maya-dex".to_string()));
        assert!(names.contains(&"xtask".to_string()));
    }
}
