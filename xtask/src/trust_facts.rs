//! `cargo xtask trust-facts` — one sheet of what the existing gates proved
//! (wave 3, W3-8).
//!
//! Runs `go-no-go`, `readiness --check`, `coverage --quiet` and
//! `proof-ratchet --check` as child processes of this same binary, and writes
//! what they said to `reports/trust-facts.md` and `reports/trust-facts.json`.
//!
//! Every line is either **evidence** — the command, the commit it ran at, and
//! the result it printed — or **GAP** with the reason. Nothing is inferred: a
//! gate that failed, needs a human, did not run, or ran at an unknown commit is
//! a GAP, and the sheet copies the gate's own words rather than restating them.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use serde::Serialize;

/// The gates collected, in sheet order: subcommand, arguments, and the claim a
/// passing run supports. `go-no-go` is split into one line per launch gate.
const GATES: &[(&str, &[&str], &str)] = &[
    ("go-no-go", &[], "Mainnet launch gates"),
    (
        "readiness",
        &["--check"],
        "READINESS.md matches the evidence on disk",
    ),
    (
        "coverage",
        &["--quiet"],
        "Reality ledger: every working/verified claim names a test",
    ),
    (
        "proof-ratchet",
        &["--check"],
        "Proof ratchet: no evidence count below its baseline",
    ),
];

/// One gate run, as captured.
pub struct GateRun {
    /// The command line, as a person would type it.
    pub command: String,
    /// The claim a passing run supports.
    pub claim: String,
    /// Exit code; `None` when the process could not be started or was killed.
    pub exit_code: Option<i32>,
    /// Standard output followed by standard error.
    pub output: String,
}

/// One line of the sheet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Fact {
    /// The gate ran at a known commit and passed.
    Evidence {
        claim: String,
        command: String,
        commit: String,
        result: String,
    },
    /// No evidence, and why.
    Gap {
        claim: String,
        command: String,
        reason: String,
    },
}

fn last_line(output: &str) -> String {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("(no output)")
        .to_string()
}

/// `go-no-go` prints `PASS|FAIL|NEEDS HUMAN  <gate>` followed by one indented
/// detail line. Returns `(verdict, gate, detail)` per gate.
fn go_no_go_lines(output: &str) -> Vec<(&'static str, String, String)> {
    let mut lines = output.lines().peekable();
    let mut out = Vec::new();
    while let Some(line) = lines.next() {
        let Some((verdict, gate)) = ["PASS", "FAIL", "NEEDS HUMAN"]
            .into_iter()
            .find_map(|v| Some((v, line.strip_prefix(v)?.strip_prefix(' ')?)))
        else {
            continue;
        };
        let detail = lines
            .next_if(|l| l.starts_with(' '))
            .map(str::trim)
            .unwrap_or_default();
        out.push((verdict, gate.trim().to_string(), detail.to_string()));
    }
    out
}

/// Turns captured runs into facts. `commit` is `None` when it is unknown, and
/// then nothing counts as evidence.
#[must_use]
pub fn collect(runs: &[GateRun], commit: Option<&str>) -> Vec<Fact> {
    let mut facts = Vec::new();
    for run in runs {
        let gap = |claim: &str, reason: String| Fact::Gap {
            claim: claim.to_string(),
            command: run.command.clone(),
            reason,
        };
        let pass = |claim: &str, result: String| match commit {
            Some(c) => Fact::Evidence {
                claim: claim.to_string(),
                command: run.command.clone(),
                commit: c.to_string(),
                result,
            },
            None => gap(
                claim,
                format!("passed, but the commit is unknown: {result}"),
            ),
        };
        let Some(code) = run.exit_code else {
            facts.push(gap(
                &run.claim,
                format!("did not run: {}", last_line(&run.output)),
            ));
            continue;
        };
        if run.command.ends_with("go-no-go") {
            let gates = go_no_go_lines(&run.output);
            if gates.is_empty() {
                facts.push(gap(
                    &run.claim,
                    format!(
                        "no gate lines in the output (exit {code}): {}",
                        last_line(&run.output)
                    ),
                ));
            }
            for (verdict, gate, detail) in gates {
                facts.push(match verdict {
                    "PASS" => pass(&gate, format!("PASS, {detail}")),
                    _ => gap(&gate, format!("{verdict}: {detail}")),
                });
            }
        } else if code == 0 {
            facts.push(pass(
                &run.claim,
                format!("PASS: {}", last_line(&run.output)),
            ));
        } else {
            facts.push(gap(
                &run.claim,
                format!("exit {code}: {}", last_line(&run.output)),
            ));
        }
    }
    facts
}

/// A markdown table cell: no pipe or newline may break the row.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace(['\r', '\n'], " ")
}

/// The markdown sheet.
#[must_use]
pub fn render_markdown(facts: &[Fact], commit: Option<&str>) -> String {
    let mut out = String::from("# Trust facts\n\n");
    let _ = writeln!(
        out,
        "Generated by `cargo xtask trust-facts` at commit `{}`; do not edit by hand. Each line is \
         evidence (the command, the commit, the result it printed) or GAP with the reason. \
         Nothing here is inferred.\n",
        commit.unwrap_or("unknown")
    );
    out.push_str("| Claim | Command | Commit | Result |\n|---|---|---|---|\n");
    let mut gaps = 0;
    for f in facts {
        let row = match f {
            Fact::Evidence {
                claim,
                command,
                commit,
                result,
            } => [
                cell(claim),
                format!("`{command}`"),
                format!("`{commit}`"),
                cell(result),
            ],
            Fact::Gap {
                claim,
                command,
                reason,
            } => {
                gaps += 1;
                [
                    cell(claim),
                    format!("`{command}`"),
                    "-".into(),
                    format!("GAP: {}", cell(reason)),
                ]
            }
        };
        let _ = writeln!(out, "| {} |", row.join(" | "));
    }
    let _ = writeln!(
        out,
        "\n**{} with evidence, {gaps} GAP.**",
        facts.len() - gaps
    );
    out
}

/// `git rev-parse HEAD`, marked when the tree has uncommitted changes (the
/// evidence is then not reproducible from the commit alone).
fn head_commit(root: &Path) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_string();
    let clean = Command::new("git")
        .args(["diff", "--quiet", "HEAD"])
        .current_dir(root)
        .status()
        .ok()?
        .success();
    Some(if clean {
        sha
    } else {
        format!("{sha}+uncommitted")
    })
}

fn run_gate(root: &Path, sub: &str, args: &[&str], claim: &str) -> GateRun {
    let command = std::iter::once(format!("cargo xtask {sub}"))
        .chain(args.iter().map(|a| (*a).to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    let result = std::env::current_exe().and_then(|exe| {
        Command::new(exe)
            .arg(sub)
            .args(args)
            .current_dir(root)
            .output()
    });
    let (exit_code, output) = match result {
        Ok(o) => (
            o.status.code(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            ),
        ),
        Err(e) => (None, e.to_string()),
    };
    GateRun {
        command,
        claim: claim.to_string(),
        exit_code,
        output,
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let out_dir = match args {
        [] => root.join("reports"),
        [flag, dir] if flag == "--out" => std::path::PathBuf::from(dir),
        _ => return Err("usage: cargo xtask trust-facts [--out <dir>]".into()),
    };
    let commit = head_commit(&root);
    let runs: Vec<GateRun> = GATES
        .iter()
        .map(|(sub, a, claim)| run_gate(&root, sub, a, claim))
        .collect();
    let facts = collect(&runs, commit.as_deref());
    let md = render_markdown(&facts, commit.as_deref());
    let json = serde_json::to_string_pretty(&serde_json::json!({
        "commit": commit,
        "facts": facts,
    }))
    .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    for (name, body) in [("trust-facts.md", md), ("trust-facts.json", json + "\n")] {
        let path = out_dir.join(name);
        std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let gaps = facts
        .iter()
        .filter(|f| matches!(f, Fact::Gap { .. }))
        .count();
    println!(
        "trust-facts: {} with evidence, {gaps} GAP -> {}",
        facts.len() - gaps,
        out_dir.join("trust-facts.md").display()
    );
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn gate(command: &str, exit_code: Option<i32>, output: &str) -> GateRun {
        GateRun {
            command: command.into(),
            claim: "the claim".into(),
            exit_code,
            output: output.into(),
        }
    }

    const GO_NO_GO: &str = "PASS         Reality ledger: every claim names a real test\n             evidence: features.toml (cargo xtask coverage)\nFAIL         External audits: at least one, zero open critical/high\n             no audit report\nNEEDS HUMAN  Legal sign-off received\n             the project owner confirms this manually\n\n1 PASS, 1 FAIL, 1 NEEDS HUMAN — NO-GO\nError: 1 launch gates fail\n";

    #[test]
    fn go_no_go_splits_into_one_fact_per_gate_and_only_pass_is_evidence() {
        let facts = collect(
            &[gate("cargo xtask go-no-go", Some(1), GO_NO_GO)],
            Some("abc"),
        );
        assert_eq!(
            facts,
            vec![
                Fact::Evidence {
                    claim: "Reality ledger: every claim names a real test".into(),
                    command: "cargo xtask go-no-go".into(),
                    commit: "abc".into(),
                    result: "PASS, evidence: features.toml (cargo xtask coverage)".into(),
                },
                Fact::Gap {
                    claim: "External audits: at least one, zero open critical/high".into(),
                    command: "cargo xtask go-no-go".into(),
                    reason: "FAIL: no audit report".into(),
                },
                Fact::Gap {
                    claim: "Legal sign-off received".into(),
                    command: "cargo xtask go-no-go".into(),
                    reason: "NEEDS HUMAN: the project owner confirms this manually".into(),
                },
            ]
        );
    }

    #[test]
    fn a_single_line_gate_passes_on_exit_zero_and_quotes_its_last_line() {
        let facts = collect(
            &[
                gate(
                    "cargo xtask proof-ratchet --check",
                    Some(0),
                    "kani 80\nproof-ratchet: no count below its baseline\n",
                ),
                gate(
                    "cargo xtask readiness --check",
                    Some(1),
                    "Error: READINESS.md is stale; run `cargo xtask readiness`\n",
                ),
            ],
            Some("abc"),
        );
        assert_eq!(
            facts[0],
            Fact::Evidence {
                claim: "the claim".into(),
                command: "cargo xtask proof-ratchet --check".into(),
                commit: "abc".into(),
                result: "PASS: proof-ratchet: no count below its baseline".into(),
            }
        );
        assert_eq!(
            facts[1],
            Fact::Gap {
                claim: "the claim".into(),
                command: "cargo xtask readiness --check".into(),
                reason: "exit 1: Error: READINESS.md is stale; run `cargo xtask readiness`".into(),
            }
        );
    }

    #[test]
    fn nothing_is_evidence_without_a_commit_or_a_run() {
        let facts = collect(
            &[
                gate("cargo xtask coverage --quiet", Some(0), "ok\n"),
                gate("cargo xtask coverage --quiet", None, "killed\n"),
                gate(
                    "cargo xtask go-no-go",
                    Some(101),
                    "thread 'main' panicked\n",
                ),
            ],
            None,
        );
        assert!(
            facts.iter().all(|f| matches!(f, Fact::Gap { .. })),
            "{facts:?}"
        );
        let reasons: Vec<_> = facts
            .iter()
            .map(|f| match f {
                Fact::Gap { reason, .. } => reason.as_str(),
                Fact::Evidence { .. } => unreachable!(),
            })
            .collect();
        assert_eq!(
            reasons,
            [
                "passed, but the commit is unknown: PASS: ok",
                "did not run: killed",
                "no gate lines in the output (exit 101): thread 'main' panicked",
            ]
        );
    }

    #[test]
    fn the_markdown_marks_gaps_and_cannot_be_broken_by_a_pipe() {
        let facts = collect(
            &[
                gate("cargo xtask coverage --quiet", Some(0), "a | b\n"),
                gate("cargo xtask go-no-go", Some(1), GO_NO_GO),
            ],
            Some("abc"),
        );
        let md = render_markdown(&facts, Some("abc"));
        assert!(
            md.contains("| the claim | `cargo xtask coverage --quiet` | `abc` | PASS: a \\| b |"),
            "{md}"
        );
        assert!(md.contains("| Legal sign-off received | `cargo xtask go-no-go` | - | GAP: NEEDS HUMAN: the project owner confirms this manually |"), "{md}");
        assert!(md.ends_with("**2 with evidence, 2 GAP.**\n"), "{md}");
    }

    #[test]
    fn the_json_tags_each_fact() {
        let facts = collect(
            &[gate("cargo xtask coverage --quiet", None, "")],
            Some("abc"),
        );
        let v = serde_json::to_value(&facts).unwrap();
        assert_eq!(v[0]["kind"], "gap");
        assert_eq!(v[0]["reason"], "did not run: (no output)");
    }
}
