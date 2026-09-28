//! `cargo xtask status` — where the project stands, from evidence on disk.
//!
//! It does not build anything by default. CLAUDE.md's "one cargo scope at a
//! time" rule means a status command that ran cargo would wedge `target/`
//! whenever another build was in flight, which is exactly when someone asks
//! for status. So it reads what the last sweep recorded and prints that
//! record's date and commit beside every figure; `--live` adds a `cargo
//! check` for anyone who wants a fresh answer and knows the tree is idle.
//!
//! Anything the evidence does not contain prints as MISSING, never as a
//! default (Operating Protocol, proof discipline 5).

use crate::evidence::{self, Sweep};
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

const MISSING: &str = "MISSING";
const SWEEP_PATH: &str = "reports/sweeps/latest.json";
const GAP_REGISTER: &str = "reports/11-gap-register.md";
/// How many "do next" items the summary shows.
const NEXT_SHOWN: usize = 3;
/// How many of STATE.md's open-gap lines the summary shows.
const GAPS_SHOWN: usize = 5;

// Every command shares `main`'s dispatch signature, so `Result` stays.
#[allow(clippy::unnecessary_wraps)]
pub fn run(args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let mut out = render(&root, &git_head(&root));
    if args.iter().any(|a| a == "--live") {
        let _ = writeln!(out, "\n## Live check\n{}", live_check(&root));
    }
    print!("{out}");
    Ok(())
}

/// The whole report, from the files under `root`. `head` is the checked-out
/// commit, passed in so tests need no git.
pub fn render(root: &Path, head: &str) -> String {
    let read = |p: &str| std::fs::read_to_string(root.join(p)).ok();
    let mut out = String::from("# Maya2C status\n");
    let state = read("STATE.md").unwrap_or_default();
    build_health(&mut out, read(SWEEP_PATH).as_deref(), head);
    gaps(&mut out, read(GAP_REGISTER).as_deref(), &state);
    ledger(&mut out, read("features.toml").as_deref());
    let _ = writeln!(
        out,
        "
## Current milestone (STATE.md)"
    );
    state_lines(&mut out, "Current milestone", &state, usize::MAX);
    let _ = writeln!(
        out,
        "
## Do next (STATE.md)"
    );
    state_lines(&mut out, "Do next", &state, NEXT_SHOWN);
    out
}

fn build_health(out: &mut String, sweep_json: Option<&str>, head: &str) {
    let _ = writeln!(out, "\n## Build health (last sweep)");
    let Some(sweep) = sweep_json.and_then(|j| serde_json::from_str::<Sweep>(j).ok()) else {
        let _ = writeln!(
            out,
            "{MISSING}: no readable {SWEEP_PATH}; run `cargo xtask sweep`"
        );
        return;
    };
    let stale = if head.starts_with(&sweep.commit) || sweep.commit.starts_with(head) {
        String::new()
    } else {
        format!(" — HEAD is now {head}; the tree has changed since")
    };
    let clean = if sweep.clean { "clean" } else { "incremental" };
    let _ = writeln!(out, "{} at {} ({clean}){stale}", sweep.date, sweep.commit);
    for s in &sweep.steps {
        let verdict = if s.ok { "ok  " } else { "FAIL" };
        let tests = s.tests.map_or(String::new(), |t| {
            format!(
                " — {} passed, {} failed, {} skipped",
                t.passed, t.failed, t.skipped
            )
        });
        let _ = writeln!(out, "  {verdict} {:<14} {:>5}s{tests}", s.name, s.seconds);
    }
}

fn gaps(out: &mut String, register: Option<&str>, state: &str) {
    let _ = writeln!(out, "\n## Open gaps");
    match register.and_then(evidence::gap_counts) {
        Some(g) => {
            let _ = writeln!(
                out,
                "gap register: {} P0, {} P1, {} P2 ({GAP_REGISTER})",
                g.p0, g.p1, g.p2
            );
        }
        None => {
            let _ = writeln!(out, "gap register: {MISSING}");
        }
    }
    state_lines(out, "Open P0 and P1 gaps", state, GAPS_SHOWN);
}

fn ledger(out: &mut String, features: Option<&str>) {
    let _ = writeln!(out, "\n## Reality ledger (features.toml subsystems)");
    match features.map(evidence::status_counts) {
        Some(Ok(counts)) => {
            let parts: Vec<String> = counts.iter().map(|(s, n)| format!("{n} {s}")).collect();
            let _ = writeln!(out, "{}", parts.join(", "));
        }
        Some(Err(e)) => {
            let _ = writeln!(out, "{MISSING}: features.toml does not parse: {e}");
        }
        None => {
            let _ = writeln!(out, "{MISSING}: no features.toml");
        }
    }
}

/// Up to `limit` lines of a STATE.md section, or MISSING.
fn state_lines(out: &mut String, heading: &str, state: &str, limit: usize) {
    let found = evidence::section(state, heading);
    if found.is_empty() {
        let _ = writeln!(out, "{MISSING}: no `## {heading}` section in STATE.md");
    }
    for l in found.iter().take(limit) {
        let _ = writeln!(out, "{l}");
    }
}

fn git_head(root: &Path) -> String {
    Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || MISSING.into(),
            |o| String::from_utf8_lossy(&o.stdout).trim().into(),
        )
}

fn live_check(root: &Path) -> String {
    match Command::new("cargo")
        .args(["check", "--workspace", "--quiet"])
        .current_dir(root)
        .status()
    {
        Ok(s) if s.success() => "cargo check --workspace: ok".into(),
        Ok(s) => format!("cargo check --workspace: FAILED ({s})"),
        Err(e) => format!("cargo check --workspace: could not run ({e})"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    /// A throwaway directory, removed on drop. Hand-rolled because xtask
    /// keeps its dependency list short (see its Cargo.toml).
    struct Dir(std::path::PathBuf);
    impl Dir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tree(files: &[(&str, &str)]) -> Dir {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir =
            Dir(std::env::temp_dir().join(format!("xtask-status-{}-{n}", std::process::id())));
        std::fs::create_dir_all(dir.path()).unwrap();
        for (path, text) in files {
            let p = dir.path().join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        dir
    }

    #[test]
    fn an_empty_tree_prints_missing_for_every_figure_and_no_numbers() {
        let dir = tree(&[]);
        let out = render(dir.path(), "abc1234");
        assert!(out.matches(MISSING).count() >= 6, "{out}");
        assert!(
            !out.contains("passed") && !out.contains(" verified"),
            "{out}"
        );
    }

    #[test]
    fn a_sweep_on_an_older_commit_is_flagged_stale() {
        let sweep = r#"{"date":"2026-09-28","commit":"old0000","clean":true,"steps":[
            {"name":"nextest","command":"x","ok":false,"seconds":9,
             "tests":{"passed":10,"failed":2,"skipped":1}}]}"#;
        let dir = tree(&[(SWEEP_PATH, sweep)]);
        let out = render(dir.path(), "new1111");
        assert!(out.contains("HEAD is now new1111"), "{out}");
        assert!(out.contains("FAIL nextest"), "{out}");
        assert!(out.contains("10 passed, 2 failed, 1 skipped"), "{out}");
    }

    #[test]
    fn milestone_gaps_and_next_tasks_come_from_state_md() {
        let state = "## Current milestone and % complete\nM1 CORE SOLID\n\
                     ## Open P0 and P1 gaps\n- P0 red CI\n\
                     ## Do next\n1. a\n2. b\n3. c\n4. d\n";
        let dir = tree(&[
            ("STATE.md", state),
            (GAP_REGISTER, "**3 items: 1 P0, 0 P1, 2 P2.**"),
        ]);
        let out = render(dir.path(), "abc");
        assert!(
            out.contains("M1 CORE SOLID") && out.contains("- P0 red CI"),
            "{out}"
        );
        assert!(out.contains("1 P0, 0 P1, 2 P2"), "{out}");
        assert!(
            out.contains("3. c") && !out.contains("4. d"),
            "top three only: {out}"
        );
    }
}
