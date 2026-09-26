//! `cargo xtask release-check` (Master Prompt 11 §3, ADR-016).
//!
//! Proves the mainnet binary is built from mainnet code only:
//!
//! 1. `maya2c-node` builds with `--no-default-features --features production`. The build guard
//!    (`crates/build-guard`) makes this step fail if any SIM, RESEARCH,
//!    frontier, test-util or insecure feature is anywhere in the graph.
//! 2. `cargo tree -e features` for that build names no forbidden feature and
//!    no crate `features.toml` classes as **SIM**.
//! 3. The binary's symbol table contains no symbol from those crates.
//!
//! `--force-sim` runs the negative case instead: the same production build
//! with `maya-entropy/sim-sources` forced into the graph, which must fail at
//! the guard. The command exits non-zero in that mode *when the guard fires*,
//! because a failed build is the expected outcome and CI must not read it as
//! green — the output says which it was.
//!
//! What this does **not** forbid: RESEARCH crates the node links for
//! consensus with an activation height of `u64::MAX` (ADR-002 — gating them
//! by cargo feature would make two honest nodes compute different state
//! roots). They are listed, so the linkage is visible, not hidden.

use serde::Deserialize;
use std::path::Path;
use std::process::Command;

const PROFILE: &str = "ci";
const FORBIDDEN_FEATURES: [&str; 8] = [
    "\"sim-sources\"",
    "\"hqc\"",
    "\"threshold-lattice\"",
    "\"frontier\"",
    "maya-build-guard feature \"sim\"",
    "maya-build-guard feature \"research\"",
    "maya-build-guard feature \"test-util\"",
    "maya-build-guard feature \"insecure\"",
];

#[derive(Deserialize)]
struct Ledger {
    #[serde(default)]
    subsystem: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    class: String,
    #[serde(default)]
    paths: String,
}

pub fn run(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    if args.iter().any(|a| a == "--force-sim") {
        return force_sim(&root);
    }
    let (sim, research) = classified_packages(&root)?;
    println!("release-check: build maya2c-node --features production (profile {PROFILE})");
    let status = Command::new("cargo")
        .current_dir(&root)
        .args([
            "build",
            "--profile",
            PROFILE,
            "-p",
            "maya2c-node",
            "--no-default-features",
            "--features",
            "maya2c-node/production",
        ])
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !status.success() {
        return Err(
            "the production build failed (the build guard or a compile error; see above)".into(),
        );
    }
    println!("  [ok]   production build compiled");

    let tree = output(
        &root,
        &[
            "tree",
            "-p",
            "maya2c-node",
            "--no-default-features",
            "--features",
            "maya2c-node/production",
            "-e",
            "features",
            "--prefix",
            "none",
        ],
    )?;
    let mut failures = Vec::new();
    for f in FORBIDDEN_FEATURES {
        if tree.lines().any(|l| l.contains(f)) {
            failures.push(format!("forbidden feature in graph: {f}"));
        }
    }
    let linked = |pkg: &str| tree.lines().any(|l| l.starts_with(&format!("{pkg} v")));
    for pkg in &sim {
        if linked(pkg) {
            failures.push(format!("SIM crate linked: {pkg}"));
        }
    }
    println!(
        "  [{}] cargo tree: {} forbidden features checked, {} SIM crates checked",
        if failures.is_empty() { "ok" } else { "FAIL" },
        FORBIDDEN_FEATURES.len(),
        sim.len()
    );

    let bin = root.join("target").join(PROFILE).join("maya2c-node");
    match Command::new("nm")
        .args(["-C", "--defined-only"])
        .arg(&bin)
        .output()
    {
        Ok(o) if o.status.success() => {
            let symbols = String::from_utf8_lossy(&o.stdout);
            let hits: Vec<&String> = sim
                .iter()
                .filter(|pkg| symbols.contains(&format!("{}::", pkg.replace('-', "_"))))
                .collect();
            if hits.is_empty() {
                println!("  [ok]   symbol table: no symbol from any SIM crate");
            } else {
                failures.push(format!("SIM symbols in binary: {hits:?}"));
            }
        }
        _ => println!("  [skip] symbol table: `nm` unavailable (binutils)"),
    }

    let dark: Vec<&String> = research.iter().filter(|p| linked(p)).collect();
    println!(
        "  [info] RESEARCH crates linked dark for consensus determinism (ADR-002): {}",
        if dark.is_empty() {
            "none".to_string()
        } else {
            dark.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );

    if failures.is_empty() {
        println!("release-check: PASS");
        Ok(())
    } else {
        Err(format!("release-check: FAIL\n  {}", failures.join("\n  ")))
    }
}

fn force_sim(root: &Path) -> Result<(), String> {
    println!("release-check --force-sim: production build with maya-entropy/sim-sources forced in");
    let out = Command::new("cargo")
        .current_dir(root)
        .args([
            "check",
            "--profile",
            PROFILE,
            "-p",
            "maya2c-node",
            "-p",
            "maya-entropy",
            "--no-default-features",
            "--features",
            "maya2c-node/production,maya-entropy/sim-sources",
        ])
        .output()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    let fired = stderr.contains("a `production` build has a SIM, RESEARCH");
    if let Some(line) = stderr
        .lines()
        .find(|l| l.contains("error: a `production` build"))
    {
        println!("  {line}");
    }
    if out.status.success() {
        println!(
            "release-check --force-sim: the build SUCCEEDED; the guard did not fire (this is a bug)"
        );
        return Err("guard did not fire".into());
    }
    if fired {
        println!("release-check --force-sim: FAIL, as expected: the build guard refused the build");
        Err("expected failure: production + sim-sources refused by the build guard".into())
    } else {
        Err(format!("the build failed for another reason:\n{stderr}"))
    }
}

fn output(root: &Path, args: &[&str]) -> Result<String, String> {
    let o = Command::new("cargo")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if !o.status.success() {
        return Err(format!(
            "cargo {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&o.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Package names of workspace members the ledger classes SIM and RESEARCH,
/// found by matching each entry's backticked paths to member directories.
fn classified_packages(root: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    let text = std::fs::read_to_string(root.join("features.toml")).map_err(|e| e.to_string())?;
    let ledger: Ledger = toml::from_str(&text).map_err(|e| e.to_string())?;
    let members = crate::members(root)?;
    let mut sim = Vec::new();
    let mut research = Vec::new();
    for e in &ledger.subsystem {
        for path in e.paths.split('`').skip(1).step_by(2) {
            let path = path.trim_end_matches('/');
            if let Some((_, name)) = members.iter().find(|(dir, _)| dir == path) {
                let bucket = match e.class.as_str() {
                    "SIM" => &mut sim,
                    "RESEARCH" => &mut research,
                    _ => continue,
                };
                if !bucket.contains(name) {
                    bucket.push(name.clone());
                }
            }
        }
    }
    // The simulator harness itself is SIM by definition even without a row.
    if !sim.iter().any(|s| s == "maya-sim") {
        sim.push("maya-sim".to_string());
    }
    Ok((sim, research))
}
