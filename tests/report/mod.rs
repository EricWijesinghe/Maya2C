//! The resilience report the chaos run writes.
//!
//! # Why a test writes a document
//!
//! A test suite answers "did anything break". A resilience report answers "what
//! is this chain's behaviour under attack", which is a different question and
//! the one an operator taking a testnet live actually has. The two answers must
//! not drift, so the report is generated from the run rather than written
//! alongside it.
//!
//! # Every row says whether a defence exists
//!
//! [`Finding::Verified`] means the chain has a defence and the run confirmed
//! it. [`Finding::Characterised`] means it has none, and the row records what
//! happens instead. A report that presented both as green ticks would be a
//! document that overstates the chain's resilience, which is worse than having
//! no document.
//!
//! # Accumulated across processes, and scoped to one run
//!
//! `cargo nextest` runs each test in its own process, so a shared in-memory
//! collector would see one row per file. Each test writes its row to
//! `<target tmpdir>/chaos-rows/<run id>/<scenario>.row` instead, and the
//! reporter reads them back.
//!
//! Two things that a first attempt got wrong, both worth stating because both
//! produced a *plausible* report rather than an obviously broken one:
//!
//! **Rows must be scoped to a run.** Without the run id in the path, a reporter
//! that ran before its siblings finished emitted a mixture of this run's rows
//! and the previous run's — a document that looked complete and was not.
//!
//! **The reporter must wait.** Under nextest there is no ordering between
//! tests, so [`write`] polls until every expected scenario has reported. If it
//! times out it still writes, but stamps the document as incomplete rather than
//! quietly under-reporting.
//!
//! The run id comes from nextest's `NEXTEST_ATTEMPT_ID`, whose prefix is a UUID
//! shared by every test in the run. Under plain `cargo test` there is one
//! process for the whole binary, so the process id serves the same purpose.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Whether the chain defended, or merely behaved predictably.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finding {
    /// A defence exists and the run confirmed it holds.
    Verified,
    /// No defence exists. The row records the observed behaviour instead.
    Characterised,
}

impl Finding {
    fn label(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::Characterised => "characterised",
        }
    }
}

/// Scenarios expected to report in a complete run.
///
/// A constant rather than a count of test functions, because nothing can count
/// those at runtime. It is what lets [`write`] tell "still waiting" apart from
/// "that is all of them", and what makes an under-reported document impossible
/// to mistake for a complete one.
pub const EXPECTED_SCENARIOS: usize = 11;

/// How long the reporter waits for its siblings.
///
/// Comfortably past the slowest network scenario. A run with a failing test
/// never reaches this count, so the wait is what that run pays before writing
/// an explicitly incomplete report.
const ASSEMBLY_TIMEOUT: Duration = Duration::from_secs(150);

/// Identifies this test run across the processes nextest spawns.
///
/// `NEXTEST_ATTEMPT_ID` is `<run uuid>:<binary>$<test>`, and the prefix is
/// shared by every test in the run. Under `cargo test` the whole binary is one
/// process, so its id is exactly as good a discriminator.
fn run_id() -> String {
    std::env::var("NEXTEST_ATTEMPT_ID")
        .ok()
        .and_then(|id| id.split(':').next().map(str::to_owned))
        .unwrap_or_else(|| format!("pid-{}", std::process::id()))
}

/// Where this run's rows are dropped.
fn rows_dir() -> PathBuf {
    // `CARGO_TARGET_TMPDIR` is given to integration tests specifically for
    // scratch files and is cleaned with `cargo clean`, unlike a path invented
    // under the repository root.
    Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("chaos-rows")
        .join(run_id())
}

/// Records one scenario's outcome.
///
/// Called by each test with what it actually asserted. Writing this from the
/// test rather than from a table at the bottom of the file is deliberate: a
/// table drifts from the assertions above it, and nothing catches that.
pub fn record(scenario: &str, finding: Finding, observed: &str) {
    let dir = rows_dir();
    if fs::create_dir_all(&dir).is_err() {
        // A report is a courtesy; failing to write one must never fail the
        // test that produced the result.
        return;
    }

    // Named by scenario so a re-run overwrites its own row rather than
    // accumulating duplicates across runs.
    let safe: String = scenario
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();

    let line = format!("{}\u{1f}{}\u{1f}{}\n", scenario, finding.label(), observed);
    let _ = fs::write(dir.join(format!("{safe}.row")), line);
}

/// One recorded row.
struct Row {
    scenario: String,
    finding: String,
    observed: String,
}

fn read_rows() -> Vec<Row> {
    let Ok(entries) = fs::read_dir(rows_dir()) else {
        return Vec::new();
    };

    let mut rows: Vec<Row> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "row"))
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .filter_map(|line| {
            let mut parts = line.trim_end().split('\u{1f}');
            Some(Row {
                scenario: parts.next()?.to_string(),
                finding: parts.next()?.to_string(),
                observed: parts.next()?.to_string(),
            })
        })
        .collect();

    // Sorted, so the document is stable between runs and a diff shows a real
    // change rather than filesystem ordering.
    rows.sort_by(|a, b| a.scenario.cmp(&b.scenario));
    rows
}

/// Renders the report and returns it.
///
/// Returns `None` when no rows were recorded — which happens when a single test
/// is run in isolation, and is not a failure.
pub fn render(seed: u64) -> Option<String> {
    let rows = read_rows();
    if rows.is_empty() {
        return None;
    }
    let complete = rows.len() >= EXPECTED_SCENARIOS;

    let verified = rows.iter().filter(|r| r.finding == "verified").count();
    let characterised = rows.len() - verified;

    let mut out = String::new();
    let _ = write!(
        out,
        r#"# Resilience report

Generated by `tests/chaos_simulator.rs`. Do not edit by hand — rerun with:

```bash
cargo test --test chaos_simulator
```

Run seed: `{seed}` (replay with `MAYA_CHAOS_SEED={seed}`).

## How to read this

Every row is one of two things, and conflating them would make this document
overstate what the chain does:

- **verified** — a defence exists and this run confirmed it holds.
- **characterised** — no defence exists. The row records the observed behaviour
  so it is a known property rather than a testnet surprise.

{rows_len} scenarios: {verified} verified, {characterised} characterised.
{completeness}
## Scenarios

| Scenario | Finding | Observed |
|---|---|---|
"#,
        rows_len = rows.len(),
        completeness = if complete {
            String::new()
        } else {
            format!(
                "
> **This report is incomplete.** {} of {EXPECTED_SCENARIOS} scenarios                  reported. A scenario that failed never records a row, so re-read the test                  output before trusting the table below.
",
                rows.len()
            )
        },
    );

    for row in &rows {
        let _ = writeln!(
            out,
            "| {} | {} | {} |",
            row.scenario, row.finding, row.observed
        );
    }

    out.push_str(
        r#"
## The characterisations, in full

**There is no orphan pool.** `Chain::insert_block` calls `require(&parent_id)?`
and errors when the parent is unknown; nothing in `src/network/node.rs` holds
the block for later. A block that arrives before its parent is therefore
discarded, not deferred, and the sender must re-send it. Gossip does not
guarantee order, so on a live testnet a node that misses a parent stalls on that
branch until another path re-delivers the child.

That is a design position, not a bug — an orphan pool is memory an unauthenticated
peer can grow. It is recorded here because it is invisible from the outside and
because "the chain reorganised slowly" is how it presents.

**Corrupted blocks are not rejected — they are indistinguishable.** A corruption
landing in a block's transactions leaves the header untouched, and `Block::id()`
hashes only the header. The chain therefore sees the honest block it already
holds and reports a duplicate. State stays correct, but no defence fired. This
is the uncommitted-transaction-list finding showing through from a different
angle, and it is why that row reads *characterised* rather than *verified*.

**Lattice proof-of-useful-work is not a live surface.** `lattice-pow` sits behind
an activation height of `u64::MAX`, nothing in `src/consensus/` calls it, and no
block carries one of these proofs. The tampering scenario exercises
`verify_solution` directly, for panic-freedom on attacker-controlled `i64`
arithmetic. It says nothing about a deployed chain, because there is nothing
deployed to say it about.

## Covered elsewhere

These were in the brief and are already tested. They are not duplicated here;
re-implementing them would create a second copy to keep in step.

| Behaviour | Where |
|---|---|
| Double-spend; two conflicting spends in one block | `tests/attack_simulation_tests.rs` |
| Invalid ML-DSA / SLH-DSA signatures, every bit of both halves flipped | `tests/malleability_tests.rs` |
| Fork choice by accumulated work; reorg atomicity on mid-reorg failure | `tests/consensus_tests.rs` |
| A block with an unknown parent is rejected | `tests/consensus_tests.rs` |
| Decoder robustness against arbitrary bytes | `fuzz/fuzz_targets/*_decode.rs` |
| Propagation under fixed latency, 0-250 ms | `tests/latency_sim_tests.rs` |

## What this run does not establish

- **Nothing here is a proof of Byzantine fault tolerance.** This chain has no
  validator set and no quorum; it is proof of work, where the security argument
  is about hash rate, not about a fraction of nodes. "2 of 6 nodes misbehave"
  tests that honest nodes ignore garbage — not that the chain tolerates a third
  of its stake defecting, which is not a statement that means anything here.
- **Six nodes on an in-process memory transport is not a network.** It models no
  bandwidth limit, no jitter, no packet loss and no queueing. A pass is evidence
  that the protocol survives the injected fault, not that it survives a bad
  network.
- **Proof-of-work verification is disabled** in the chain-level scenarios. They
  test the state transition engine, not the hasher; `tests/dag_tests.rs` covers
  that.
"#,
    );

    Some(out)
}

/// Writes the report to `docs/benchmarks/resilience-report.md`.
///
/// # Errors
///
/// Any filesystem error, which the caller should surface rather than ignore —
/// a report that silently failed to write is a report somebody will read a
/// stale copy of.
pub fn write(seed: u64) -> std::io::Result<Option<PathBuf>> {
    // Wait for the siblings. Under nextest this process may well start before
    // the scenarios it is reporting on have run at all.
    let deadline = Instant::now() + ASSEMBLY_TIMEOUT;
    while read_rows().len() < EXPECTED_SCENARIOS && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }

    let Some(body) = render(seed) else {
        return Ok(None);
    };

    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs")
        .join("benchmarks")
        .join("resilience-report.md");

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, body)?;
    Ok(Some(path))
}
