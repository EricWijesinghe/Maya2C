# Backlog

Work found during a session that is bigger than the session. Each item
names its priority band (CLAUDE.md, Operating Protocol, "Choosing what to
do next") and where it came from. Take from the top of the highest open
band; move an item to PROGRESS.md when it starts.

## P2 — make claimed-working features verified

- **Measure line coverage of the core crates** with `cargo llvm-cov`
  (installed, 0.8.7; never run). M1 criterion 8. Source: EMPIRE.md.
- **Property tests for fees, consensus and VM.** State has a seeded
  property test (`crates/node/tests/supply_property_tests.rs`); a search
  found none for `fee-market`, `dag-bft` or `vm`. First confirm by reading
  their test suites, then add seeded property tests. M1 criterion 5.
- **Gap-register allowlist.** `scripts/gap_register.py` counts reviewed
  guards as P0 (DECISIONS.md, 2026-09-28). Add a reviewed-guard marker so
  the count means open work. Port to Rust (`cargo xtask gaps`) at the same
  time: new tooling here is Rust.
- **Re-check that every number in reports 12, 13, 14, 26** carries commit,
  hardware, OS and command (Production Standing Orders). M2.
- **PROGRESS.md is stale in places**: "Phase B — physical layout — not
  started" although the tree already has `crates/ bins/ apps/ hal/`; the
  section header still names branch `claude/task-0g86kl`, which was merged
  and deleted. Reconcile against git history.

## P3 — current milestone (M1)

- Decide MP06's scope for M1 (EMPIRE.md, Q1) — Eric.
- `cargo xtask bench` with commit/hardware/OS/command stamped into every
  result (CLAUDE.md says it does not exist).

## P4 — adoption-critical

- MP23: move the safe-contract resource rules from opt-in imports onto the
  consensus import surface (needs an ADR and invariant work).
- MP24: guest traps mapped to source lines (needs a backtrace out of the
  consensus execution path without changing results).
- MP22/MP29/MP24 usability and developer studies — need participants (Eric).

## P5 — differentiators

- MP25: zero-knowledge light client (research-sized).
- MP28: outside-asset route into PQ vaults (depends on MP25); archive task
  sealing.
- Maya Chat difference: dated searches for PQ authentication and
  chain-account identity (docs/prior-art/p2p-chat.md).

## P6 — deferred and frontier (untouched until their milestone)

- MP06: dealer-free MPC offline phase; per-order dark-pool proofs;
  proof-of-personhood liveness and uniqueness (needs attested hardware,
  invariant 11); Landsat second oracle sensor (requester-pays bucket).
- MP24: lazy fork state — conflicts with invariant 24; not to be built as
  specified.
