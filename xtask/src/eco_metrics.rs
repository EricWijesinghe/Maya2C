//! `cargo xtask eco-metrics` — the ecosystem metrics that can be computed
//! today, and a named gap for each one that cannot (Master Prompt 30 §4).
//!
//! Methods are in `docs/ecosystem/METRICS.md`. In short:
//!
//! - **Active developers per month.** Distinct commit author emails in this
//!   repository's history per calendar month (UTC). Agent and bot identities
//!   (`noreply@anthropic.com`, any `[bot]` or `noreply@github.com` address)
//!   are counted separately, never as developers.
//! - **Retention.** A human author is *eligible* at N days once their first
//!   commit is at least N days before the newest commit, and *retained* if
//!   they committed again at least N days after their first.
//! - **Contracts.** Contract crates under `contracts/`. "Deployed and used"
//!   needs chain data, and there is no public testnet.
//!
//! No email address is printed: the output is counts only, so it can be
//! published as it stands.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

const DAY: i64 = 86_400;

/// One commit: author email and Unix time.
struct Commit {
    email: String,
    at: i64,
}

fn is_agent(email: &str) -> bool {
    email == "noreply@anthropic.com"
        || email.contains("[bot]")
        || email.ends_with("noreply@github.com")
}

fn commits(root: &Path) -> Result<Vec<Commit>, String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(["log", "--format=%ae%x09%at"])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err("git log failed".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (email, at) = l.split_once('\t')?;
            Some(Commit {
                email: email.to_ascii_lowercase(),
                at: at.parse().ok()?,
            })
        })
        .collect())
}

/// `YYYY-MM` for a Unix time, from the civil-from-days algorithm (no clock,
/// no time zone database: UTC throughout).
fn month(at: i64) -> String {
    let z = at.div_euclid(DAY) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}")
}

/// `(eligible, retained)` human authors at `days`.
fn retention(firsts: &BTreeMap<&str, (i64, i64)>, newest: i64, days: i64) -> (usize, usize) {
    let eligible: Vec<_> = firsts
        .values()
        .filter(|(first, _)| newest - first >= days * DAY)
        .collect();
    let retained = eligible
        .iter()
        .filter(|(first, last)| last - first >= days * DAY)
        .count();
    (eligible.len(), retained)
}

/// Whether the clone lacks older history, which truncates every figure here.
fn shallow(root: &Path) -> bool {
    Command::new("git")
        .current_dir(root)
        .args(["rev-parse", "--is-shallow-repository"])
        .output()
        .is_ok_and(|o| o.stdout.starts_with(b"true"))
}

fn contract_crates(root: &Path) -> usize {
    std::fs::read_dir(root.join("contracts")).map_or(0, |d| {
        d.flatten()
            .filter(|e| e.path().join("Cargo.toml").exists())
            .count()
    })
}

/// Prints the metrics table.
pub fn run(_args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let all = commits(&root)?;
    let newest = all.iter().map(|c| c.at).max().ok_or("no commits")?;
    let mut per_month: BTreeMap<String, (BTreeSet<&str>, BTreeSet<&str>)> = BTreeMap::new();
    let mut firsts: BTreeMap<&str, (i64, i64)> = BTreeMap::new();
    for c in &all {
        let slot = per_month.entry(month(c.at)).or_default();
        if is_agent(&c.email) {
            slot.1.insert(&c.email);
            continue;
        }
        slot.0.insert(&c.email);
        let e = firsts.entry(&c.email).or_insert((c.at, c.at));
        e.0 = e.0.min(c.at);
        e.1 = e.1.max(c.at);
    }
    println!("| Metric | Value | Source |\n|---|---|---|");
    for (m, (humans, agents)) in &per_month {
        println!(
            "| Active developers, {m} | {} human, {} agent identities | git history |",
            humans.len(),
            agents.len()
        );
    }
    let span_days = (newest - all.iter().map(|c| c.at).min().unwrap_or(newest)) / DAY;
    let truncated = if shallow(&root) {
        " (**shallow clone: older history missing**; run in a full clone)"
    } else {
        ""
    };
    println!(
        "| History covered | {} commits over {span_days} days{truncated} | git history |",
        all.len()
    );
    for days in [30, 90] {
        let (eligible, retained) = retention(&firsts, newest, days);
        let value = if eligible == 0 {
            format!("not yet measurable: no author's first commit is {days} days old")
        } else {
            format!("{retained} of {eligible}")
        };
        println!("| Retention at {days} days | {value} | git history |");
    }
    println!(
        "| Contract crates in tree | {} | contracts/ |",
        contract_crates(&root)
    );
    for (name, why) in [
        (
            "Contracts deployed and actively used",
            "no public testnet to read",
        ),
        (
            "Time to first deploy",
            "no deploy flow outside tests; the course's first cohort measures it",
        ),
        (
            "SDK downloads",
            "no SDK is published (publishing needs approval)",
        ),
        ("Grant outcomes", "the grants program is not approved"),
    ] {
        println!("| {name} | **gap**: {why} | — |");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_are_computed_in_utc_across_boundaries() {
        assert_eq!(month(0), "1970-01");
        assert_eq!(month(951_782_400), "2000-02"); // 2000-02-29T00:00Z, a leap day
        assert_eq!(month(1_790_812_799), "2026-09"); // 2026-09-30T23:59:59Z
        assert_eq!(month(1_790_812_800), "2026-10");
    }

    #[test]
    fn retention_counts_only_eligible_authors() {
        let mut f = BTreeMap::new();
        f.insert("a", (0, 40 * DAY)); // came back after 40 days
        f.insert("b", (0, DAY)); // one-off
        f.insert("c", (95 * DAY, 95 * DAY)); // too new to judge
        assert_eq!(retention(&f, 100 * DAY, 30), (2, 1));
        assert_eq!(retention(&f, 100 * DAY, 90), (2, 0));
    }

    #[test]
    fn agents_are_not_developers() {
        assert!(is_agent("noreply@anthropic.com"));
        assert!(is_agent("dependabot[bot]@users.noreply.github.com"));
        assert!(!is_agent("someone@example.org"));
    }
}
