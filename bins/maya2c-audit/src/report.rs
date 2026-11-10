//! The signed-off-able report: one finding per check, each with the heights it
//! rests on, and an exit code a script can branch on.

use serde::Serialize;

/// Lines kept per finding. A failing run over a long window stops listing
/// after this, and says how many it left out.
const MAX_LINES: usize = 50;

/// How a check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Every observation agreed with the spec.
    Pass,
    /// An observation disagreed. The lines name the heights.
    Fail,
    /// The node does not keep the data, or the window cannot close the check.
    /// Not a pass. Never reported as one.
    Unavailable,
    /// Does not apply to this chain (for example no mining, so no emission).
    Skipped,
}

impl Verdict {
    fn label(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Unavailable => "NOT VERIFIED",
            Self::Skipped => "NOT APPLICABLE",
        }
    }

    /// The worse of two verdicts: Fail beats Unavailable beats the rest.
    #[must_use]
    pub fn worst(self, other: Self) -> Self {
        let rank = |v: Self| match v {
            Self::Fail => 3,
            Self::Unavailable => 2,
            Self::Pass => 1,
            Self::Skipped => 0,
        };
        if rank(other) > rank(self) {
            other
        } else {
            self
        }
    }
}

/// One check's result and the evidence behind it.
#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub id: &'static str,
    pub title: &'static str,
    pub verdict: Verdict,
    pub lines: Vec<String>,
    /// Lines beyond `MAX_LINES`, counted, not printed.
    pub omitted: usize,
}

impl Finding {
    #[must_use]
    pub fn new(id: &'static str, title: &'static str, verdict: Verdict) -> Self {
        Self {
            id,
            title,
            verdict,
            lines: Vec::new(),
            omitted: 0,
        }
    }

    /// Adds evidence. Past `MAX_LINES`, the line is counted instead of kept.
    pub fn note(&mut self, line: impl Into<String>) {
        if self.lines.len() < MAX_LINES {
            self.lines.push(line.into());
        } else {
            self.omitted += 1;
        }
    }
}

/// The whole run.
#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub tool: &'static str,
    pub tip: u64,
    pub from: u64,
    pub to: u64,
    pub findings: Vec<Finding>,
    /// What the tool does not check. Printed, never counted as a pass.
    pub not_verified_here: &'static [&'static str],
}

impl Report {
    /// The worst verdict among the findings. Skipped when nothing applied.
    #[must_use]
    pub fn overall(&self) -> Verdict {
        self.findings
            .iter()
            .fold(Verdict::Skipped, |acc, f| acc.worst(f.verdict))
    }

    /// 0 when nothing failed or was left unverified, 1 on a failure, 3 when
    /// something could not be verified. 2 is reserved for the CLI's own errors.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self.overall() {
            Verdict::Fail => 1,
            Verdict::Unavailable => 3,
            Verdict::Pass | Verdict::Skipped => 0,
        }
    }

    /// Plain text for a person to read and sign off.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = format!(
            "{}: tip {}, window {}..={}\n",
            self.tool, self.tip, self.from, self.to
        );
        for f in &self.findings {
            out.push_str(&format!("[{}] {} {}\n", f.verdict.label(), f.id, f.title));
            for line in &f.lines {
                out.push_str(&format!("    {line}\n"));
            }
            if f.omitted > 0 {
                out.push_str(&format!("    ... {} more not shown\n", f.omitted));
            }
        }
        out.push_str(&format!("RESULT: {}\n", self.overall().label()));
        out.push_str("Not verified by this tool:\n");
        for item in self.not_verified_here {
            out.push_str(&format!("  - {item}\n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn report(verdicts: &[Verdict]) -> Report {
        Report {
            tool: "t",
            tip: 1,
            from: 1,
            to: 1,
            findings: verdicts
                .iter()
                .map(|v| Finding::new("x", "x", *v))
                .collect(),
            not_verified_here: &[],
        }
    }

    #[test]
    fn a_failure_outranks_unverified_which_outranks_pass() {
        use Verdict::*;
        assert_eq!(report(&[Pass, Fail, Unavailable]).overall(), Fail);
        assert_eq!(report(&[Pass, Unavailable]).overall(), Unavailable);
        assert_eq!(report(&[Pass, Skipped]).overall(), Pass);
        assert_eq!(report(&[]).overall(), Skipped);
    }

    #[test]
    fn exit_codes_separate_fail_from_unverified() {
        use Verdict::*;
        assert_eq!(report(&[Pass]).exit_code(), 0);
        assert_eq!(report(&[Fail]).exit_code(), 1);
        assert_eq!(report(&[Unavailable]).exit_code(), 3);
    }

    #[test]
    fn long_evidence_is_counted_not_dropped_silently() {
        let mut f = Finding::new("x", "x", Verdict::Fail);
        for i in 0..(MAX_LINES + 3) {
            f.note(format!("line {i}"));
        }
        assert_eq!(f.lines.len(), MAX_LINES);
        assert_eq!(f.omitted, 3);
    }
}
