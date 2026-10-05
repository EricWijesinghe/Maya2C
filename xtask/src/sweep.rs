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
/// Numbered re-run records per day before falling back to `-partial`.
const MAX_RERUNS_PER_DAY: u32 = 999;

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
    write_reports(&root, &sweep, &tails, !only.is_empty())
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
    let (program, shown) = match program {
        "SELF" => (exe.to_string_lossy().into_owned(), "cargo xtask"),
        "bash" => (bash(), "bash"),
        other => (other.to_string(), other),
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
    // Plain output: the parsers match text, and an escape code beside
    // `passed` or `Finished` would silently lose a count.
    let run = Command::new(program)
        .args(args)
        .current_dir(root)
        .env("CARGO_TERM_COLOR", "never")
        .env("NO_COLOR", "1")
        .output();
    match run {
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

/// A partial run (named steps only) is written as `<date>-<n>` and never
/// replaces `latest.json`: a two-step rerun is not a sweep, and `status`
/// must not report it as one.
fn write_reports(root: &Path, sweep: &Sweep, tails: &str, partial: bool) -> Result<(), String> {
    let dir = root.join("reports/sweeps");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem = if partial {
        free_stem(&dir, &sweep.date)
    } else {
        sweep.date.clone()
    };
    let json = serde_json::to_string_pretty(sweep).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{stem}.json")), &json).map_err(|e| e.to_string())?;
    if !partial {
        std::fs::write(dir.join("latest.json"), &json).map_err(|e| e.to_string())?;
    }
    let mut md = format!(
        "# Verification sweep {stem}\n\nGenerated by `cargo xtask sweep{}` at commit `{}`. \
         Each block is the last {TAIL_LINES} lines of the command's real output; \
         full logs were written to the target directory's `sweep/`.\n\n\
         | Step | Result | Seconds | Tests |\n|---|---|---|---|\n",
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
    std::fs::write(dir.join(format!("{stem}.md")), md).map_err(|e| e.to_string())?;
    let failed = sweep.steps.iter().filter(|s| !s.ok).count();
    println!(
        "sweep: {} steps, {failed} failed — reports/sweeps/{stem}.md",
        sweep.steps.len()
    );
    Ok(())
}

/// The bash the scripts expect: one with this machine's cargo on PATH.
///
/// Windows starts a child by looking in System32 before PATH, so a plain
/// `bash` from a Windows program is WSL's, which has no cargo; the first
/// sweep's lint ratchet "passed" with zero diagnostics that way. So on
/// Windows: `SWEEP_BASH` if set, else the first `bash.exe` on PATH outside
/// `System32` and `WindowsApps`, else Git's own, found beside `git.exe`.
fn bash() -> String {
    if let Some(explicit) = std::env::var_os("SWEEP_BASH") {
        return explicit.to_string_lossy().into_owned();
    }
    if !cfg!(windows) {
        return "bash".into();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    let usable = |d: &&PathBuf| {
        let s = d.to_string_lossy().to_ascii_lowercase();
        !s.contains("system32") && !s.contains("windowsapps")
    };
    let on_path = dirs.iter().filter(usable).map(|d| d.join("bash.exe"));
    let beside_git = dirs
        .iter()
        .filter(|d| d.join("git.exe").is_file())
        .filter_map(|d| d.parent().map(|p| p.join("bin").join("bash.exe")));
    on_path
        .chain(beside_git)
        .find(|p| p.is_file())
        .map_or_else(|| "bash".into(), |p| p.to_string_lossy().into_owned())
}

/// `<date>-<n>` with the first `n` not yet used, so a second re-run on the
/// same day does not overwrite the first.
pub(crate) fn free_stem(dir: &Path, date: &str) -> String {
    let names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    (1..=MAX_RERUNS_PER_DAY)
        .map(|n| format!("{date}-{n}"))
        .find(|stem| {
            let (dash, dot) = (format!("{stem}-"), format!("{stem}."));
            !names
                .iter()
                .any(|n| n.starts_with(&dash) || n.starts_with(&dot))
        })
        .unwrap_or_else(|| format!("{date}-partial"))
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
pub(crate) fn today() -> String {
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
