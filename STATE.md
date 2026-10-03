# State

## Handover
- **Done (2026-10-03):** gate 1 (ADR-036, PR #43) and gate 2 (ADR-037, PR #44) finished in code with full workspace runs green; Wasmtime 48.0.4 (#42); wallet redesign with Receive, live network pill and Sent screen (#45, e2e green); testnet watchdog task. Report: `reports/sessions/2026-10-03-gates-1-2-wallet.md`.
- **Unfinished:** merge order #42 → #43 → #44 → #45 (each retargeted to master as its base merges), plus #32, #30, #35, #39 after updating against master; then rebuild testnet binaries from master and re-genesis maya-testnet-1 (third time) — the 7-day soak (gate 3) restarts there.
- **Do first next session:** `gh pr checks 42 43`; merge what is green in that order; then the re-genesis runbook (ADR-036 consequences) and update docs/site join-page genesis values.
- **Done (2026-09-29):** https://maya2c.dev live (GitHub Pages, certificate issued, HTTPS enforced). PR #10 has the quickstart, signatures and mining guides (each run first), `go get maya2c.dev/sdk`, `infra/oracle-free` (not applied, ADR-032) and `release-binaries.yml`. Report: `reports/sessions/2026-09-29-launch.md`.
- **Unfinished:** none from this session. #10 merged; `release-binaries` built all four targets, aarch64 included (run 36478279508). PR #8 (installer) and #9 (site visuals) belong to the deploy session.
- **Done (2026-09-29, later):** Eric approved the launch steps ("Everything is approved"). Apache-2.0 LICENSE (#18). Wallets pay fees correctly and reach the gateway's JSON-RPC at `/rpc` (#17). The gateway has CORS (#22) and per-client rate limiting (#30). "Join the testnet" page with faucet form (#24). maya-testnet-1 runs on Eric's PC from `D:\Maya2C-testnet` (node, gateway, faucet with journal, chat relay over WSS; `start-testnet.ps1` runs at logon).
- **Blocked on Eric:** the Cloudflare Tunnel authorization (`cloudflared tunnel login` timed out three times); after it: rpc/faucet/chat.maya2c.dev, then tag `node-v0.1.0-testnet.1`. An Oracle Cloud account to move the seed off the PC. Mining vs ADR-016 (no block reward) is still undecided.
- **Done (2026-09-28):** one branch (`master`); CI fixed; operating system in place (`cargo xtask status`/`sweep`, MISSION, EMPIRE, ECOSYSTEM, RISKS, DECISIONS, BACKLOG, Operating Protocol); first clean sweep: 2,972 passed, 0 failed; two gates that were silently wrong fixed (lint ratchet false pass, doc-coverage red).
- **Unfinished:** GitHub CI for `2748cb5` (review fixes to xtask/lint script) was queued at handover; `master` runs cancel each other on new pushes, so the last *completed* full CI is PR #3's (21 checks green, contains `69073e3`). Maya Chat merged (PR #3) — Eric's decision on its difference still open.
- **Do first next session:** `cargo xtask status`; `gh run list --branch master` — if anything is red, fix it (P0); else "Do next" item 1.

## Current milestone and % complete by evidence
**M1 CORE SOLID** — M0 TRUTH met. M1 exit criteria (docs/EMPIRE.md): 6 of 10 met by evidence (build, tests, gates, fuzzing, formal/invariants, MP12 baseline — criteria 1, 2, 3, 6, 7, 9); criterion 4 (CI green) pending one job; open: 5 (property tests for fees/consensus/VM), 8 (line coverage), 10 (MP06 scope, Eric).

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
