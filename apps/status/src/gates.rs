//! Mainnet launch gates, as recorded in `docs/mainnet-v1-plan.md`.
//!
//! The states are written by an operator from that document, not computed:
//! a gate's evidence is a report in the repository, and no code can read
//! "the security review came back clean". The test below holds the list to
//! the document's own table, so a gate cannot silently go missing.

use serde::{Deserialize, Serialize};

/// The bundled list, parsed by [`parse`] at start.
pub const BUNDLED: &str = include_str!("../data/gates.json");

/// Where a gate stands, as the plan document records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Recorded as met.
    Met,
    /// Some of it is done; the document names what is left.
    Partial,
    /// Not started, or no evidence yet.
    Open,
}

/// One launch gate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    /// The gate's number in the plan's table.
    pub number: u32,
    /// Short name.
    pub title: String,
    /// Where it stands.
    pub state: State,
    /// One line on the evidence or what is missing.
    pub note: String,
    /// Repository path of the plan section or report, if any.
    pub evidence: Option<String>,
}

/// How many gates are in each state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    /// Gates met.
    pub met: usize,
    /// Gates partly met.
    pub partial: usize,
    /// Gates open.
    pub open: usize,
    /// All gates.
    pub total: usize,
}

/// The gates in JSON form.
///
/// # Errors
///
/// The JSON is malformed or a gate number repeats.
pub fn parse(text: &str) -> Result<Vec<Gate>, String> {
    let gates: Vec<Gate> = serde_json::from_str(text).map_err(|e| format!("gates: {e}"))?;
    let mut numbers: Vec<u32> = gates.iter().map(|g| g.number).collect();
    numbers.sort_unstable();
    if numbers.windows(2).any(|w| w[0] == w[1]) {
        return Err("gates: a gate number repeats".into());
    }
    Ok(gates)
}

/// Counts of each state.
#[must_use]
pub fn progress(gates: &[Gate]) -> Progress {
    let count = |s: State| gates.iter().filter(|g| g.state == s).count();
    Progress {
        met: count(State::Met),
        partial: count(State::Partial),
        open: count(State::Open),
        total: gates.len(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// Gate numbers in the plan's table, read from the document itself.
    fn document_gate_numbers() -> Vec<u32> {
        include_str!("../../../docs/mainnet-v1-plan.md")
            .lines()
            .skip_while(|l| !l.starts_with("| # | Gate |"))
            .skip(2)
            .take_while(|l| l.starts_with('|'))
            .filter_map(|l| l.split('|').nth(1)?.trim().parse().ok())
            .collect()
    }

    #[test]
    fn the_bundled_list_covers_every_gate_in_the_plan() {
        let gates = parse(BUNDLED).unwrap();
        let mut ours: Vec<u32> = gates.iter().map(|g| g.number).collect();
        ours.sort_unstable();
        let mut theirs = document_gate_numbers();
        theirs.sort_unstable();
        assert_eq!(
            theirs.len(),
            10,
            "the plan's table changed shape; update data/gates.json"
        );
        assert_eq!(ours, theirs);
    }

    #[test]
    fn a_repeated_gate_number_is_refused() {
        let one = r#"{"number":1,"title":"a","state":"met","note":"n","evidence":null}"#;
        assert!(parse(&format!("[{one},{one}]")).is_err());
    }

    #[test]
    fn progress_counts_each_state() {
        let gates = parse(BUNDLED).unwrap();
        let p = progress(&gates);
        assert_eq!(p.met + p.partial + p.open, p.total);
        assert_eq!(p.total, gates.len());
    }
}
