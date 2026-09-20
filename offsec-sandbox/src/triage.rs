//! Turning a finding into something a human can act on: minimize, classify,
//! dedup, and emit a regression-test stub.
//!
//! It never writes a patch. A patch to consensus code is a human decision — the
//! same line invariant 13 draws for governance — and synthesizing one from a
//! crash is unsolved. What this does is the tractable, honest part: shrink the
//! input to the smallest that still triggers the same class of outcome, give it
//! a stable signature so duplicates collapse, and write a failing test in the
//! shape of `tests/exploit_replays.rs` that asserts the *reason* the input is
//! handled once it is fixed.

use crate::Surface;
use crate::oracle::{self, Outcome};

/// A minimized, classified finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The surface the input was mutated for.
    pub surface: Surface,
    /// The minimized input that still triggers the outcome.
    pub input: Vec<u8>,
    /// What class of failure it is.
    pub kind: Kind,
    /// The oracle outcome, verbatim.
    pub outcome: Outcome,
}

/// The class of a finding, which is also its dedup bucket together with the
/// signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A panic on the decode or apply path: denial of service.
    Panic,
    /// The same block produced two roots: a consensus split.
    Divergence,
}

impl Kind {
    fn of(outcome: &Outcome) -> Option<Self> {
        match outcome {
            Outcome::Panicked(_) => Some(Self::Panic),
            Outcome::Diverged { .. } => Some(Self::Divergence),
            Outcome::Clean | Outcome::Deterministic => None,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::Divergence => "divergence",
        }
    }
}

/// Builds a finding from an input the oracle flagged, minimizing it first.
///
/// Returns `None` if `input` does not actually reproduce a finding — a caller
/// should only reach here when the oracle said it does, but re-checking keeps a
/// flaky report from becoming a committed stub.
#[must_use]
pub fn triage(surface: Surface, input: &[u8]) -> Option<Finding> {
    let outcome = oracle::check(surface, input);
    let kind = Kind::of(&outcome)?;
    let minimized = minimize(surface, input, kind);
    Some(Finding {
        surface,
        outcome: oracle::check(surface, &minimized),
        input: minimized,
        kind,
    })
}

/// Shrinks `input` toward the smallest input with the same finding kind, by
/// repeated halving and byte trimming. Bounded, deterministic, and never
/// enlarges the input.
#[must_use]
pub fn minimize(surface: Surface, input: &[u8], kind: Kind) -> Vec<u8> {
    let reproduces = |candidate: &[u8]| Kind::of(&oracle::check(surface, candidate)) == Some(kind);

    let mut best = input.to_vec();
    // A fixed number of passes: each pass tries to drop a chunk. Halving first
    // finds large removable regions cheaply; the tail trims the edges.
    for _ in 0..16 {
        let mut shrunk = false;
        let mut chunk = best.len() / 2;
        while chunk >= 1 {
            let mut i = 0;
            while i < best.len() {
                let end = (i + chunk).min(best.len());
                let mut candidate = Vec::with_capacity(best.len() - (end - i));
                candidate.extend_from_slice(&best[..i]);
                candidate.extend_from_slice(&best[end..]);
                if candidate.len() < best.len() && reproduces(&candidate) {
                    best = candidate;
                    shrunk = true;
                } else {
                    i += chunk;
                }
            }
            chunk /= 2;
        }
        if !shrunk {
            break;
        }
    }
    best
}

impl Finding {
    /// A stable signature, so two findings with the same cause collapse to one.
    ///
    /// For a panic it is the panic message with addresses and lengths stripped;
    /// for a divergence it is the pair of roots. The surface and kind prefix it,
    /// so a panic and a divergence on the same bytes are two findings.
    #[must_use]
    pub fn signature(&self) -> String {
        let detail = match &self.outcome {
            Outcome::Panicked(message) => normalize(message),
            Outcome::Diverged { left, right } => format!("{left}!={right}"),
            Outcome::Clean | Outcome::Deterministic => "none".to_string(),
        };
        format!("{}/{}/{detail}", self.surface.name(), self.kind.label())
    }

    /// A regression test in the shape of `tests/exploit_replays.rs`: it pins the
    /// minimized input as a constant and asserts the *reason* it is now handled
    /// — a clean refusal or agreeing roots — not merely that it stopped
    /// crashing. A human moves it into the suite and writes the fix.
    #[must_use]
    pub fn regression_stub(&self) -> String {
        let name = format!(
            "{}_{}_{}",
            self.surface.name(),
            self.kind.label(),
            short_hash(&self.input)
        );
        let assertion = match self.kind {
            Kind::Panic => {
                "        // Before the fix this input panicked. Assert the *reason* it is\n        // now safe — a clean `Err`, or a value the chain accepts — not just\n        // that it no longer crashes.\n        let outcome = check(SURFACE, INPUT);\n        assert!(!outcome.is_finding(), \"still a finding: {outcome:?}\");"
            }
            Kind::Divergence => {
                "        // Before the fix this block produced two different state roots.\n        // Assert the roots now agree, which is what invariant 24 requires.\n        let outcome = check(SURFACE, INPUT);\n        assert!(matches!(outcome, Outcome::Deterministic | Outcome::Clean), \"still diverges: {outcome:?}\");"
            }
        };
        format!(
            "// Auto-generated regression stub. Move into tests/exploit_replays.rs,\n// keep the input, and write the fix. Do not commit this file as-is.\n#[test]\nfn regression_{name}() {{\n    use maya_offsec_sandbox::oracle::{{check, Outcome}};\n    use maya_offsec_sandbox::Surface;\n    const SURFACE: Surface = Surface::{surface:?};\n    const INPUT: &[u8] = &{input:?};\n    {{\n{assertion}\n    }}\n}}\n",
            surface = self.surface,
            input = self.input,
        )
    }
}

/// Strips addresses, long hex runs and standalone numbers from a panic message
/// so two panics at the same site with different values collapse.
fn normalize(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut chars = message.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '0' && chars.peek() == Some(&'x') {
            chars.next();
            while chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                chars.next();
            }
            out.push_str("0x_");
        } else if c.is_ascii_digit() {
            while chars.peek().is_some_and(|d| d.is_ascii_digit()) {
                chars.next();
            }
            out.push('#');
        } else {
            out.push(c);
        }
    }
    out
}

/// A short, stable hex tag for a byte string, for naming a stub.
fn short_hash(bytes: &[u8]) -> String {
    // FNV-1a: enough to name a file, not a cryptographic claim.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_clean_input_yields_no_finding() {
        oracle::quiet_panics();
        assert!(triage(Surface::Transaction, &[0u8; 4]).is_none());
    }

    #[test]
    fn normalize_collapses_addresses_and_numbers() {
        assert_eq!(
            normalize("index out of bounds: the len is 3 but the index is 0x7ffddeadbeef"),
            "index out of bounds: the len is # but the index is 0x_"
        );
    }

    #[test]
    fn minimize_never_enlarges_and_is_deterministic() {
        oracle::quiet_panics();
        // A synthetic "finding": treat any input containing 0xFF as a panic by
        // routing through a kind check on a fabricated outcome. Here we just
        // assert the shrink loop's shape on a clean input — it returns the
        // input unchanged when nothing reproduces.
        let input = vec![1u8; 64];
        let out = minimize(Surface::Transaction, &input, Kind::Panic);
        assert!(out.len() <= input.len());
        assert_eq!(out, minimize(Surface::Transaction, &input, Kind::Panic));
    }

    #[test]
    fn a_stub_names_its_surface_and_kind() {
        let finding = Finding {
            surface: Surface::Wasm,
            input: vec![0xde, 0xad],
            kind: Kind::Panic,
            outcome: Outcome::Panicked("boom".to_string()),
        };
        let stub = finding.regression_stub();
        assert!(stub.contains("Surface::Wasm"));
        assert!(stub.contains("regression_wasm_panic_"));
        assert!(stub.contains("[222, 173]"));
        assert!(finding.signature().starts_with("wasm/panic/"));
    }
}
