# State

## Handover
- **Done (2026-09-28):** one branch (`master`); CI fix pushed (`06202e1`); operating system set up — `cargo xtask status` and `cargo xtask sweep`, MISSION, EMPIRE, ECOSYSTEM, RISKS, DECISIONS, BACKLOG, Operating Protocol in CLAUDE.md.
- **Unfinished:** first clean verification sweep running; this file's build-health section is filled from it when it finishes.
- **Do first next session:** `cargo xtask status`; if CI on `master` is not green, fix it (P0); otherwise the top of "Do next".

## Current milestone and % complete by evidence
**M1 CORE SOLID** (M0 TRUTH met). Exit criteria met by evidence: pending the sweep — see [docs/EMPIRE.md](docs/EMPIRE.md) §M1.

## Build health
Pending the first sweep (`reports/sweeps/`). Last recorded full run before it:
`cargo nextest run --workspace` 2,961 passed, 0 failed, 11 skipped — `reports/31-handover.md` §6 (warm, not clean).

## Open P0 and P1 gaps
- P0 — CI on `master`: rustfmt and unsafe accounting were red; fix pushed in `06202e1`, not yet observed green.
- Gap register: 3 P0 rows, all reviewed guards, not work (DECISIONS.md, 2026-09-28); 0 P1.

## Do next
1. Confirm CI green on `master` after `06202e1` (P0).
2. Measure core line coverage with `cargo llvm-cov` (M1 criterion 8, P2).
3. Confirm or add property tests for fees, consensus, VM (M1 criterion 5, P2).

## Blocked on Eric
- **Maya Chat's difference.** PQ encryption already ships in Signal, iMessage, SimpleX (`docs/prior-art/p2p-chat.md`). Chat is being built on `feat/utopia-chat`; the plan's rule says an app without an honest answer is not started. Decide: continue, and on which claimed difference.
- **EMPIRE Q1:** does M1 need all of MP06 or only its ADR-016 core parts? (Recommended: core parts.)
- **Everything in `reports/31-handover.md` §4:** audits, testnet approvals, benchmark runner, study participants, signing certificate.

## Do not touch
- P6 deferred and frontier modules (BACKLOG.md P6) until their milestone.
- MP24 lazy fork state — conflicts with invariant 24.
- `apps/chat`, `docs/adr/ADR-031*`, `crates/crypto-pq` — being changed by the chat session on `feat/utopia-chat`.
