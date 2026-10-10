# State

## Handover
- **Done (2026-10-10, recovery after the Nemotron week):**
  - The maya2c.dev alert was a false positive (Cloudflare email obfuscation). The
    watcher is fixed. Eric confirmed Cloudflare is clear.
  - Harness settings restored, and the non-existent MCP entries removed.
  - Nemotron's tree audited (PR #108, branch `fix/post-nemotron-recovery`):
    - attacknet faults are now honest;
    - the broken lab scripts are quarantined;
    - the property tests are fixed;
    - lint debt is 794.
    - Workspace: 3159 passed.
  - ADR-043: the seed, killed by the lab and 13 epochs behind, could not
    rejoin. Code fixed:
    - `--catch-up-from` is repeatable and picks the live peer with the
      highest tip;
    - validators snapshot by default.
    - Testnet scripts updated to match.
  - Report: `reports/sessions/2026-10-10-recovery.md`.
- **Unfinished:**
  - PR #108 awaiting CI. 9 commits:
    - recovery;
    - ADR-043 code;
    - attacknet attack 7;
    - p2p simultaneous-start fix;
    - review fixes.
  - Gate 3 soak running since 2026-10-10 19:25 KST.
  - Seed fully back: epoch 110, committee 4, voting.
  - The testnet binaries are still the old build. Deploy the #108 build
    after merge, one validator at a time, only while the committee is 4.
  - BACKLOG: `--min-epoch` floor, two-source agreement, snapshot metering.
- **Do first next session:**
  - `gh pr checks 108`; merge when green.
  - Then rolling-deploy the new node build to the testnet, v4 → v3 → v2 →
    seed, checking committee 4 between each step.
  - Gate 4 needs the Oracle VMs (Eric).

- **Done (2026-10-04/05):**
  - Site: responsive and light-mode fix, live (#81).
  - Status page (#75), bootstrap gateway (#76), published genesis
    (#80, #83).
  - ADR-040 part 2: stake-weighted genesis (#77).
  - `cargo xtask attacknet`, with 8 rejoin and robustness bugs fixed (#85).
  - Eric decided that checkpoints keep n − f, a >2/3 trust floor.
  - `scripts/join-testnet.sh`; Dependabot.
  - Release `node-v0.1.0-testnet.2` tagged.
  - Seed redeployed from master 9917db56, with `node-config.toml` raising
    the node RPC limit to 1000/s.
  - 12 project peers in `D:\Maya2C-peers` (`start-peers.ps1`, process
    name `maya2c-peer`).
  - Report: `reports/sessions/2026-10-05-attacknet.md`.
- **Unfinished:**
  - Release run for testnet.2: confirm the assets, then smoke-test
    `join-testnet.sh` in WSL against it.
  - #79 (nightly link check): merge when green.
  - Peer 1's public re-bootstrap: confirm it reaches the tip.
- **Do first next session:**
  - Check the release, then the `join-testnet.sh` smoke test.
  - Gate 10: more than f validators that each genuinely need to follow.
    Design followers that fetch missing blocks over p2p, cross-checked
    against an n − f checkpoint. Write the ADR first.
  - KNOWN_ISSUES 20: observers follow over p2p, not RPC polling.

- **Done (2026-10-04 overnight):**
  - explorer.maya2c.dev live.
  - Testnet on master with ADR-038 checkpoints, `get_bft_status`, and the
    `--min-register-bond 1000000000` floor.
  - Quorum is n − f (ADR-039, #54).
  - Validator tooling and guide (#55, #58).
  - Live consensus cells, SEO card, NLnet refresh.
  - First release tagged: `node-v0.1.0-testnet.1`.
  - Report: `reports/sessions/2026-10-04-overnight.md`.
- **Unfinished:**
  - Release run 37166896701: confirm the assets, and that `releases/latest`
    resolves.
  - #59 (ADR-040 part 1) awaiting CI.
  - #21 Astro bump needs a visual check.
- **Do first next session:**
  - Check the release run, then merge #59 when green.
  - Then Eric's decisions:
    - ADR-040 part 2 (weights at mainnet genesis?);
    - P2P reachability (router port 31100, Oracle VM, or WebSocket
      transport);
    - submit NLnet.
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
