//! `cargo xtask sweep [--clean]` — the verification sweep (Operating
//! Protocol, proof discipline 4).
//!
//! Runs every gate this repository has, one cargo scope at a time and in a
//! fixed order, and records what each proved: `reports/sweeps/<date>.json`
//! (and `latest.json`, which `status` reads) plus `<date>.md` with the tail
//! of each command's real output. Full logs go under the target directory,
//! not the repo.
//!
//! It does not stop at the first failure. A sweep's job is to report the
//! state of everything; a red build still leaves the ledger and the scripts
//! worth checking. `--clean` runs `cargo clean` first, which is what makes a
//! sweep's numbers mean "from scratch".
//!
//! The ledger, spec and readiness gates run through this same executable
//! rather than `cargo xtask`: a nested `cargo run -p xtask` would try to
//! rebuild the binary that is running, which Windows refuses.

use crate::evidence::{self, Step, Sweep};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Lines of each command's output kept in the committed report.
const TAIL_LINES: usize = 25;
const SECONDS_PER_DAY: u64 = 86_400;

/// `(name, program, args)`. `SELF` is replaced by this executable's path.
const STEPS: &[(&str, &str, &[&str])] = &[
    ("fmt", "cargo", &["fmt", "--all", "--check"]),
    ("unsafe", "bash", &["scripts/check-unsafe.sh"]),
    ("build", "cargo", &["build", "--workspace", "--all-targets"]),
    (
        "clippy",
        "cargo",
        &[
            "clippy",
            "--workspace",
            "--lib",
            "--bins",
            "--",
            "-D",
            "clippy::unwrap_used",
        ],
    ),
    (
        "nextest",
        "cargo",
        &["nextest", "run", "--workspace", "--no-fail-fast"],
    ),
    (
        "ledger",
        "SELF",
        &["coverage", "--quiet", "--verify-targets"],
    ),
    ("spec", "SELF", &["spec-coverage"]),
    ("readiness", "SELF", &["readiness", "--check"]),
    ("lint-debt", "bash", &["scripts/lint_debt.sh", "--check"]),
    (
        "doc-coverage",
        "bash",
        &["scripts/doc_coverage.sh", "--check"],
    ),
    ("deny", "cargo", &["deny", "check"]),
];

pub fn run(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let clean = args.iter().any(|a| a == "--clean");
    let only: Vec<&str> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(String::as_str)
        .collect();
    if clean {
        println!("sweep: cargo clean");
        let (ok, text) = execute(&root, "cargo", &["clean"]);
        if !ok {
            return Err(format!("cargo clean failed:\n{text}"));
        }
    }
    // After the clean, which deletes the target directory and would take
    // the log directory with it.
    let logs = target_dir(&root).join("sweep");
    std::fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    let mut sweep = Sweep {
        date: today(),
        commit: git_head(&root),
        clean,
        steps: Vec::new(),
    };
    let mut tails = String::new();
    for (name, program, step_args) in STEPS {
        if !only.is_empty() && !only.contains(name) {
            continue;
        }
        let step = run_step(&root, &logs, name, program, step_args, &mut tails)?;
        println!(
            "sweep: {} {name} ({}s)",
            if step.ok { "ok  " } else { "FAIL" },
            step.seconds
        );
        sweep.steps.push(step);
    }
    write_reports(&root, &sweep, &tails)
}

fn run_step(
    root: &Path,
    logs: &Path,
    name: &str,
    program: &str,
    args: &[&str],
    tails: &mut String,
) -> Result<Step, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let (program, shown) = if program == "SELF" {
        (exe.to_string_lossy().into_owned(), "cargo xtask")
    } else {
        (program.to_string(), program)
    };
    let command = format!("{shown} {}", args.join(" "));
    println!("sweep: {command}");
    let started = Instant::now();
    let (ok, text) = execute(root, &program, args);
    let seconds = started.elapsed().as_secs();
    std::fs::write(logs.join(format!("{name}.log")), &text).map_err(|e| e.to_string())?;
    let tests = if name == "nextest" {
        evidence::nextest_counts(&text)
    } else {
        None
    };
    let tail: Vec<&str> = text.lines().rev().take(TAIL_LINES).collect();
    let _ = writeln!(
        tails,
        "\n### {name} — {}\n\n```\n$ {command}",
        if ok { "ok" } else { "FAILED" }
    );
    for line in tail.iter().rev() {
        let _ = writeln!(tails, "{line}");
    }
    let _ = writeln!(tails, "```");
    Ok(Step {
        name: name.into(),
        command,
        ok,
        seconds,
        tests,
    })
}

/// Runs `program` in `root`, returning success and stdout followed by stderr.
/// A program that cannot be started is a failed step, not an error: `cargo
/// deny` missing on a machine is a finding for the report.
fn execute(root: &Path, program: &str, args: &[&str]) -> (bool, String) {
    match Command::new(program).args(args).current_dir(root).output() {
        Ok(out) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            (out.status.success(), text)
        }
        Err(e) => (false, format!("could not start `{program}`: {e}")),
    }
}

fn write_reports(root: &Path, sweep: &Sweep, tails: &str) -> Result<(), String> {
    let dir = root.join("reports/sweeps");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(sweep).map_err(|e| e.to_string())?;
    for name in [format!("{}.json", sweep.date), "latest.json".into()] {
        std::fs::write(dir.join(name), &json).map_err(|e| e.to_string())?;
    }
    let mut md = format!(
        "# Verification sweep {}\n\nGenerated by `cargo xtask sweep{}` at commit `{}`. \
         Each block is the last {TAIL_LINES} lines of the command's real output; \
         full logs were written to the target directory's `sweep/`.\n\n\
         | Step | Result | Seconds | Tests |\n|---|---|---|---|\n",
        sweep.date,
        if sweep.clean { " --clean" } else { "" },
        sweep.commit
    );
    for s in &sweep.steps {
        let tests = s.tests.map_or("—".into(), |t| {
            format!(
                "{} passed, {} failed, {} skipped",
                t.passed, t.failed, t.skipped
            )
        });
        let _ = writeln!(
            md,
            "| {} | {} | {} | {tests} |",
            s.name,
            if s.ok { "ok" } else { "**FAIL**" },
            s.seconds
        );
    }
    md.push_str(tails);
    std::fs::write(dir.join(format!("{}.md", sweep.date)), md).map_err(|e| e.to_string())?;
    let failed = sweep.steps.iter().filter(|s| !s.ok).count();
    println!(
        "sweep: {} steps, {failed} failed — reports/sweeps/{}.md",
        sweep.steps.len(),
        sweep.date
    );
    Ok(())
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), PathBuf::from)
}

fn git_head(root: &Path) -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || "unknown".into(),
            |o| String::from_utf8_lossy(&o.stdout).trim().into(),
        )
}

/// Today's UTC date as `YYYY-MM-DD`, without a date crate.
fn today() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    civil_date(secs / SECONDS_PER_DAY)
}

/// Days since 1970-01-01 to a proleptic-Gregorian date (Howard Hinnant's
/// `civil_from_days`).
fn civil_date(days: u64) -> String {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::civil_date;

    #[test]
    fn civil_date_matches_known_days() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(11_016), "2000-02-29");
        assert_eq!(civil_date(20_724), "2026-09-28");
    }
}
