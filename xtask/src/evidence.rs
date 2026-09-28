//! Parsers for the evidence files `status` reads and `sweep` writes.
//!
//! Kept free of I/O so each rule can be tested against a fixed string: the
//! whole point of `status` is that it never prints a number the evidence does
//! not contain, and a parser that guesses would defeat that.

use serde::{Deserialize, Serialize};

/// One command the sweep ran, and what it proved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub name: String,
    pub command: String,
    pub ok: bool,
    pub seconds: u64,
    /// Present only for a step whose output carried a test summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<TestCounts>,
}

/// A sweep: every step, the commit it ran on, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sweep {
    pub date: String,
    pub commit: String,
    pub clean: bool,
    pub steps: Vec<Step>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestCounts {
    pub passed: u64,
    pub failed: u64,
    pub skipped: u64,
}

/// Counts from the gap register's bold summary line
/// (`**27 items: 3 P0, 0 P1, 24 P2.**`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapCounts {
    pub p0: u64,
    pub p1: u64,
    pub p2: u64,
}

/// The number written immediately before `word` on `line`, if there is one.
/// `"12 passed (3 slow), 0 failed"` gives 12 for `passed`, 0 for `failed`.
fn number_before(line: &str, word: &str) -> Option<u64> {
    let at = line.find(word)?;
    let head = line[..at].trim_end();
    let digits: String = head
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    digits.parse().ok()
}

/// The last `Summary` line nextest printed. `None` when there is none: a run
/// that died while building has no counts, and must not be reported as zero.
pub fn nextest_counts(output: &str) -> Option<TestCounts> {
    let line = output
        .lines()
        .rev()
        .find(|l| l.contains("Summary") && l.contains("run:"))?;
    let tail = &line[line.find("run:")?..];
    Some(TestCounts {
        passed: number_before(tail, " passed")?,
        failed: number_before(tail, " failed").unwrap_or(0),
        skipped: number_before(tail, " skipped").unwrap_or(0),
    })
}

/// The gap register's summary counts, or `None` if the line is missing.
pub fn gap_counts(register: &str) -> Option<GapCounts> {
    let line = register.lines().find(|l| l.contains(" items:"))?;
    Some(GapCounts {
        p0: number_before(line, " P0")?,
        p1: number_before(line, " P1")?,
        p2: number_before(line, " P2")?,
    })
}

/// The non-empty lines of a `## heading` section of a markdown file, up to
/// the next `## `. Matching is on the heading's prefix, so
/// `"Current milestone"` finds `## Current milestone and % complete`.
pub fn section<'a>(markdown: &'a str, heading: &str) -> Vec<&'a str> {
    let prefix = format!("## {heading}");
    markdown
        .lines()
        .skip_while(|l| !l.starts_with(&prefix))
        .skip(1)
        .take_while(|l| !l.starts_with("## "))
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

/// `(status, count)` over `features.toml`'s subsystems, in a fixed order.
pub fn status_counts(features_toml: &str) -> Result<Vec<(String, usize)>, String> {
    #[derive(Deserialize)]
    struct Ledger {
        #[serde(default)]
        subsystem: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        status: String,
    }
    let ledger: Ledger = toml::from_str(features_toml).map_err(|e| e.to_string())?;
    Ok(["verified", "working", "stub", "planned"]
        .iter()
        .map(|s| {
            let n = ledger.subsystem.iter().filter(|e| e.status == *s).count();
            ((*s).to_string(), n)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn nextest_summary_counts_are_read_from_the_last_summary_line() {
        let out = "   PASS a\n     Summary [ 41.0s] 2626 tests run: 2620 passed (4 slow), \
                   3 failed, 6 skipped\n";
        assert_eq!(
            nextest_counts(out),
            Some(TestCounts {
                passed: 2620,
                failed: 3,
                skipped: 6
            })
        );
    }

    #[test]
    fn a_run_without_a_summary_has_no_counts_rather_than_zero() {
        assert_eq!(nextest_counts("error: could not compile `node`"), None);
    }

    #[test]
    fn a_summary_without_failures_reads_as_zero_failed() {
        let out = "Summary [0.3s] 1 test run: 1 passed, 0 skipped";
        assert_eq!(
            nextest_counts(out),
            Some(TestCounts {
                passed: 1,
                failed: 0,
                skipped: 0
            })
        );
    }

    #[test]
    fn gap_register_counts_come_from_its_summary_line() {
        let md = "# Gap register\n\n**27 items: 3 P0, 0 P1, 24 P2.**\n";
        assert_eq!(
            gap_counts(md),
            Some(GapCounts {
                p0: 3,
                p1: 0,
                p2: 24
            })
        );
        assert_eq!(gap_counts("# empty"), None);
    }

    #[test]
    fn section_stops_at_the_next_heading_and_matches_by_prefix() {
        let md = "## Handover\nx\n## Current milestone and % complete\n\nM1 — 40%\n\n## Next\ny";
        assert_eq!(section(md, "Current milestone"), vec!["M1 — 40%"]);
        assert!(section(md, "Missing").is_empty());
    }

    #[test]
    fn status_counts_count_subsystems_only() {
        let toml = "[[feature]]\nstatus = \"verified\"\n\
                    [[subsystem]]\nstatus = \"verified\"\n\
                    [[subsystem]]\nstatus = \"planned\"\n";
        let counts = status_counts(toml).unwrap();
        assert_eq!(counts[0], ("verified".into(), 1));
        assert_eq!(counts[3], ("planned".into(), 1));
    }

    #[test]
    fn a_sweep_round_trips_through_json() {
        let sweep = Sweep {
            date: "2026-09-28".into(),
            commit: "abc1234".into(),
            clean: true,
            steps: vec![Step {
                name: "nextest".into(),
                command: "cargo nextest run".into(),
                ok: true,
                seconds: 5,
                tests: Some(TestCounts {
                    passed: 1,
                    failed: 0,
                    skipped: 0,
                }),
            }],
        };
        let json = serde_json::to_string(&sweep).unwrap();
        assert_eq!(serde_json::from_str::<Sweep>(&json).unwrap(), sweep);
    }
}
