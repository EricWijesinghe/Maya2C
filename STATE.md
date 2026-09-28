# State

## Handover
- **Done (2026-09-28):** one branch (`master`); CI fixed; operating system in place (`cargo xtask status`/`sweep`, MISSION, EMPIRE, ECOSYSTEM, RISKS, DECISIONS, BACKLOG, Operating Protocol); first clean sweep: 2,972 passed, 0 failed; two gates that were silently wrong fixed (lint ratchet false pass, doc-coverage red).
- **Unfinished:** GitHub CI for `2748cb5` (review fixes to xtask/lint script) was queued at handover; `master` runs cancel each other on new pushes, so the last *completed* full CI is PR #3's (21 checks green, contains `69073e3`). Maya Chat merged (PR #3) — Eric's decision on its difference still open.
- **Do first next session:** `cargo xtask status`; `gh run list --branch master` — if anything is red, fix it (P0); else "Do next" item 1.

## Current milestone and % complete by evidence
**M1 CORE SOLID** — M0 TRUTH met. M1 exit criteria (docs/EMPIRE.md): 6 of 10 met by evidence (build, tests, gates, CI, fuzzing, formal/invariants, MP12 baseline — criteria 1, 2, 3, 6, 7, 9); criterion 4 (CI green) pending one job; open: 5 (property tests for fees/consensus/VM), 8 (line coverage), 10 (MP06 scope, Eric).

## Build health
Clean sweep 2026-09-28 at `71c926a` — `reports/00-operating-baseline.md`, `reports/sweeps/2026-09-28.md`:
- build from clean 1,392 s; clippy gate ok; fmt ok; unsafe ok; ledger, spec, readiness ok; deny ok
- `Summary [ 883.401s] 2972 tests run: 2972 passed (11 slow), 11 skipped`
- lint ratchet (re-run, Git bash): `lint_debt: 1140 diagnostics (baseline 1140)`
- doc coverage (after link fix): `TOTAL 9719 9692 99%`, ok
Machine: Core Ultra 9 275HX, 31.4 GB, Windows 11 build 29671, rustc 1.99.0-nightly 2026-07-14.

## Open P0 and P1 gaps
- P0 — none open locally. Confirm CI on `master` at `2748cb5` or later finishes green.
- P1 — gap register: 3 P0 rows, all reviewed guards, not work (DECISIONS.md); 0 P1.
- P2 — core line coverage never measured (M1 #8).
- P2 — property tests for fees, consensus, VM not found (M1 #5).
- P2 — doc links are checked only nightly (35 min); a per-PR check is in BACKLOG.md.

## Do next
1. Measure core line coverage with `cargo llvm-cov` and record it (M1 #8, P2).
2. Read the fee-market, dag-bft and vm test suites; add seeded property tests where none exist (M1 #5, P2).
3. Reconcile PROGRESS.md with git history; gap register in Rust that tells reviewed guards from gaps (P2).

## Blocked on Eric
- **Maya Chat's difference.** PQ encryption already ships in Signal, iMessage, SimpleX (`docs/prior-art/p2p-chat.md`). Candidates: PQ identity signatures; chat identity = chain account. Decide which claim chat stands on (its first version is merged, PR #3).
- **EMPIRE Q1:** M1 needs all of MP06, or only its ADR-016 core parts? (Recommended: core parts.)
- **`reports/31-handover.md` §4:** audits, testnet approvals, benchmark runner, study participants, signing certificate.

## Do not touch
- P6 deferred and frontier modules (BACKLOG.md P6) until their milestone.
- MP24 lazy fork state — conflicts with invariant 24.
- `apps/chat`, ADR-031 — owned by the chat session (its own worktree).
