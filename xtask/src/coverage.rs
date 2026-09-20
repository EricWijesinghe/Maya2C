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
    #[serde(default)]
    feature: Vec<Feature>,
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
    let root = crate::workspace_root();

    let path = root.join("features.toml");
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let ledger: Ledger =
        toml::from_str(&text).map_err(|e| format!("features.toml is not valid: {e}"))?;

    let packages = workspace_packages(&root)?;
    let mut problems: Vec<String> = Vec::new();
    let mut seen: BTreeMap<&str, ()> = BTreeMap::new();

    for f in &ledger.feature {
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
        print_table(&ledger.feature);
    }

    if problems.is_empty() {
        println!(
            "\nfeatures.toml: {} entries, all claims backed.",
            ledger.feature.len()
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
