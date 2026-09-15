//! The autonomous fuzzing loop, engine-independent.
//!
//! This is the red-teaming loop the brief asks for, minus coverage guidance:
//! load the seed corpus, mutate, run the [`oracle`](crate::oracle), and when an
//! input is flagged, [`triage`](crate::triage) it and write both the input and
//! a regression stub. The LibAFL engine (`--features libafl`) replaces the
//! random search with a coverage-guided one but reuses these same mutators,
//! oracle and triage.
//!
//! Deterministic: a `seed` reproduces a whole run, so a finding can always be
//! reproduced from the run that found it.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rand_core::SeedableRng;

use crate::triage::Finding;
use crate::{Rng, Surface, mutate, oracle, triage};

/// How a run is bounded.
#[derive(Clone, Copy, Debug)]
pub enum Budget {
    /// Stop after this many inputs.
    Iterations(u64),
    /// Stop after this much wall-clock time.
    Time(Duration),
}

/// A run's configuration.
#[derive(Clone, Debug)]
pub struct Config {
    /// The surface to fuzz.
    pub surface: Surface,
    /// The run seed; the same seed replays the run.
    pub seed: u64,
    /// When to stop.
    pub budget: Budget,
    /// Where to write findings (input + stub), created if absent.
    pub out_dir: PathBuf,
}

/// What a run found.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Inputs executed.
    pub executed: u64,
    /// Distinct findings kept, by signature.
    pub findings: Vec<Finding>,
}

/// Runs the loop described by `config`.
///
/// # Errors
///
/// Returns an I/O error only if a finding cannot be written; the loop itself
/// never propagates a fuzz failure, it records it.
pub fn run(config: &Config) -> std::io::Result<Report> {
    oracle::quiet_panics();
    fs::create_dir_all(&config.out_dir)?;

    let mut rng = Rng::seed_from_u64(config.seed);
    let corpus = seed_corpus(config.surface);
    let mut report = Report::default();
    let mut seen = std::collections::HashSet::new();
    let start = Instant::now();

    loop {
        if done(config.budget, report.executed, start) {
            break;
        }
        // Pick a seed and mutate it. An empty corpus still works: the byte
        // mutator generates from nothing.
        let base = pick(&corpus, &mut rng);
        let input = mutate::mutate(config.surface, &base, &mut rng);
        report.executed += 1;

        let outcome = oracle::check(config.surface, &input);
        if !outcome.is_finding() {
            continue;
        }
        let Some(finding) = triage::triage(config.surface, &input) else {
            // The flag did not reproduce on re-check: flaky, not recorded.
            continue;
        };
        if seen.insert(finding.signature()) {
            write_finding(&config.out_dir, &finding)?;
            report.findings.push(finding);
        }
    }
    Ok(report)
}

fn done(budget: Budget, executed: u64, start: Instant) -> bool {
    match budget {
        Budget::Iterations(n) => executed >= n,
        Budget::Time(limit) => start.elapsed() >= limit,
    }
}

fn pick(corpus: &[Vec<u8>], rng: &mut Rng) -> Vec<u8> {
    use rand_core::RngCore;
    if corpus.is_empty() {
        return Vec::new();
    }
    corpus[(rng.next_u32() as usize) % corpus.len()].clone()
}

/// The seed corpus for a surface: the mutators' own seeds, plus any files under
/// `fuzz/corpus/<surface>` if the tree is laid out that way.
fn seed_corpus(surface: Surface) -> Vec<Vec<u8>> {
    let mut corpus = mutate::seeds(surface);
    let shared = Path::new("../fuzz/corpus").join(surface.name());
    if let Ok(entries) = fs::read_dir(shared) {
        for entry in entries.flatten() {
            if let Ok(bytes) = fs::read(entry.path()) {
                corpus.push(bytes);
            }
        }
    }
    corpus
}

fn write_finding(out_dir: &Path, finding: &Finding) -> std::io::Result<()> {
    let stem = finding.signature().replace(['/', ' ', ':', '!'], "_");
    let stem: String = stem.chars().take(80).collect();
    fs::write(out_dir.join(format!("{stem}.input")), &finding.input)?;
    fs::write(out_dir.join(format!("{stem}.rs")), finding.regression_stub())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_transaction_run_completes_and_finds_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = run(&Config {
            surface: Surface::Transaction,
            seed: 1,
            budget: Budget::Iterations(2_000),
            out_dir: dir.path().to_path_buf(),
        })
        .expect("run");
        assert_eq!(report.executed, 2_000);
        // The tx decoder and apply path are already hardened; a finding here
        // would be a real regression, so assert none rather than tolerate some.
        assert!(report.findings.is_empty(), "unexpected findings: {:?}", report.findings);
    }

    #[test]
    fn a_run_replays_from_its_seed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cfg = |seed| Config {
            surface: Surface::Handshake,
            seed,
            budget: Budget::Iterations(500),
            out_dir: dir.path().to_path_buf(),
        };
        let a = run(&cfg(42)).expect("a");
        let b = run(&cfg(42)).expect("b");
        assert_eq!(a.executed, b.executed);
        assert_eq!(a.findings.len(), b.findings.len());
    }
}
