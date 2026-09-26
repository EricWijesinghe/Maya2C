//! `cargo xtask go-no-go` — the mainnet launch gates, from evidence
//! (Master Prompt 20 §3).
//!
//! Every gate is computed here from something on disk, never read from the
//! State column of `LAUNCH.md`. A gate prints **PASS** only with the path of
//! the evidence that passed it; **FAIL** with the reason; **NEEDS HUMAN** when
//! only a person can confirm it (legal sign-off, a staffed rota). Exits
//! non-zero while any gate fails: that is a NO-GO.

use std::path::Path;

enum Verdict {
    Pass(String),
    Fail(String),
    Human(String),
}

fn read(root: &Path, rel: &str) -> Option<String> {
    std::fs::read_to_string(root.join(rel)).ok()
}

/// Days since 1970-01-01 for a `YYYY-MM-DD` date (proleptic Gregorian).
fn days(date: &str) -> Option<i64> {
    let mut it = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, d) = (it.next()??, it.next()??, it.next()??);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn today() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    i64::try_from(secs / 86_400).unwrap_or(0)
}

/// A report's `**Date:** YYYY-MM-DD`, if it has one.
fn report_date(root: &Path, rel: &str) -> Option<i64> {
    let text = read(root, rel)?;
    let at = text.find("**Date:** ")? + "**Date:** ".len();
    days(text.get(at..at + 10)?)
}

fn recent(root: &Path, rel: &str, what: &str) -> Verdict {
    match report_date(root, rel) {
        Some(d) if today() - d <= 30 => {
            Verdict::Pass(format!("{rel} ({what}, {} days old)", today() - d))
        }
        Some(d) => Verdict::Fail(format!(
            "{what} last recorded {} days ago in {rel}; must be within 30",
            today() - d
        )),
        None => Verdict::Fail(format!("no dated record of {what}")),
    }
}

fn findings(root: &Path) -> Verdict {
    let Some(text) = read(root, "docs/audit/FINDINGS.md") else {
        return Verdict::Fail("no findings tracker (docs/audit/FINDINGS.md)".into());
    };
    let rows: Vec<&str> = text.lines().filter(|l| l.starts_with("| F-")).collect();
    if rows.is_empty() {
        return Verdict::Fail("no external audit has reported: the tracker has no findings, which is not the same as zero".into());
    }
    let open_high = rows.iter().filter(|r| {
        let l = r.to_ascii_lowercase();
        (l.contains("| critical |") || l.contains("| high |")) && !l.contains("| closed |")
    });
    let n = open_high.count();
    if n > 0 {
        Verdict::Fail(format!("{n} open critical/high findings"))
    } else {
        Verdict::Pass("docs/audit/FINDINGS.md (no open critical/high)".into())
    }
}

fn absent(root: &Path, rel: &str, what: &str) -> Verdict {
    if root.join(rel).exists() {
        Verdict::Human(format!("{rel} exists; a person must judge whether {what}"))
    } else {
        Verdict::Fail(format!("no evidence of {what} ({rel} does not exist)"))
    }
}

/// Gates about the build, the spec and the protocol's completeness.
fn build_gates(root: &Path) -> Vec<(&'static str, Verdict)> {
    let node_toml = read(root, "crates/node/Cargo.toml").unwrap_or_default();
    let ledger = read(root, "features.toml").unwrap_or_default();
    // The node links the fee market only as a dev-dependency today, so no
    // fee is charged on any network. The gate looks at the normal
    // [dependencies] section, where charging would have to start.
    let node_deps = node_toml.split("[dev-dependencies]").next().unwrap_or("");
    let fee_linked = node_deps.contains("maya-fee-market");
    vec![
        (
            "Reality ledger: every claim names a real test",
            match crate::coverage::run(&["--quiet".to_string()]) {
                Ok(()) => Verdict::Pass("features.toml (cargo xtask coverage)".into()),
                Err(e) => Verdict::Fail(e),
            },
        ),
        (
            "Spec: no consensus rule without vectors",
            match crate::spec_coverage::run(&["--strict".to_string(), "--quiet".to_string()]) {
                Ok(()) => Verdict::Pass("spec/ (cargo xtask spec-coverage --strict)".into()),
                Err(e) => Verdict::Fail(e),
            },
        ),
        (
            "SLOs: metric, dashboard, alert, runbook for each",
            match crate::slo_check::run(&["--quiet".to_string()]) {
                Ok(()) => Verdict::Pass("docs/SLO.md (cargo xtask slo-check)".into()),
                Err(e) => Verdict::Fail(e),
            },
        ),
        (
            "DAG-BFT wired into the node",
            if node_toml.contains("maya-dag-bft") {
                Verdict::Pass("crates/node/Cargo.toml".into())
            } else {
                Verdict::Fail("the node does not depend on dag-bft (ADR-015)".into())
            },
        ),
        (
            "Staking and slashing built",
            if ledger.contains("id = \"staking") {
                Verdict::Human("features.toml has a staking entry; check its status".into())
            } else {
                Verdict::Fail("no staking subsystem in features.toml".into())
            },
        ),
        (
            "Fee market active",
            if fee_linked {
                Verdict::Human(
                    "the node links maya-fee-market; confirm its activation height is finite"
                        .into(),
                )
            } else {
                Verdict::Fail("the node does not link maya-fee-market outside tests: no fee is charged on any network".into())
            },
        ),
        (
            "External audits: at least one, zero open critical/high",
            findings(root),
        ),
    ]
}

/// Gates about the network programme and its rehearsals.
fn programme_gates(root: &Path) -> Vec<(&'static str, Verdict)> {
    vec![
        (
            "State sync rehearsed in the last 30 days",
            recent(root, "reports/14-scale.md", "100M-account state sync"),
        ),
        (
            "Crash restore rehearsed in the last 30 days",
            recent(root, "reports/12-performance.md", "1,000 kill -9 restarts"),
        ),
        (
            "Coordinated restart rehearsed in the last 30 days",
            recent(
                root,
                "reports/19-operations.md",
                "12-node coordinated restart",
            ),
        ),
        (
            "Attacknet ran ≥ 4 weeks without an unresolved halt or fork",
            absent(root, "reports/attacknet.md", "an attacknet ran"),
        ),
        (
            "Incentivized testnet met SLOs ≥ 4 consecutive weeks",
            absent(
                root,
                "reports/incentivized-testnet.md",
                "an incentivized testnet ran",
            ),
        ),
        (
            "External validators across enough providers and countries",
            absent(
                root,
                "reports/validator-diversity.md",
                "an external validator set exists",
            ),
        ),
        (
            "Genesis parameters frozen in a signed file",
            absent(
                root,
                "genesis/mainnet.json.sig",
                "genesis is frozen and signed",
            ),
        ),
        (
            "Incident-response on-call rota staffed",
            Verdict::Human(
                "confirm the rota in docs/security/INCIDENT_RESPONSE.md is staffed".into(),
            ),
        ),
        (
            "Legal sign-off received",
            Verdict::Human("the project owner confirms this manually".into()),
        ),
    ]
}

pub fn run(_args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let (mut pass, mut fail, mut human) = (0, 0, 0);
    for (gate, v) in build_gates(&root).into_iter().chain(programme_gates(&root)) {
        match v {
            Verdict::Pass(ev) => {
                pass += 1;
                println!("PASS         {gate}\n             evidence: {ev}");
            }
            Verdict::Fail(why) => {
                fail += 1;
                println!("FAIL         {gate}\n             {why}");
            }
            Verdict::Human(what) => {
                human += 1;
                println!("NEEDS HUMAN  {gate}\n             {what}");
            }
        }
    }
    println!(
        "\n{pass} PASS, {fail} FAIL, {human} NEEDS HUMAN — {}",
        if fail == 0 && human == 0 {
            "GO"
        } else {
            "NO-GO"
        }
    );
    if fail > 0 {
        return Err(format!("{fail} launch gates fail"));
    }
    Ok(())
}
