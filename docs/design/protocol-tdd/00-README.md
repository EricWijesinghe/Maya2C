# Maya2C Protocol Technical Design Document (TDD)

Answer to [docs/master-prompts/protocol-taskforce.md](../../master-prompts/protocol-taskforce.md)
(Eric, 2026-11-08: "do all I said, we stay beyond everyone"). Written 2026-11-08.

**How to read the numbers.** Every number is one of three kinds, and is labelled:
**MEASURED** (a file in `reports/` is cited), **COMPUTED** (arithmetic from cited
sizes, shown), **TARGET** (a goal; Standing Order 2). Nothing in this document is
called "first", "only" or "unprecedented" (Standing Order 10). "Beyond everyone" is
the goal; the matrix in [01](01-threat-model.md) says, row by row, where the evidence
for that stands today.

## Files

| # | File | Brief sections covered |
|---|---|---|
| 00 | this file | Executive summary, scoring method, scorecard, build order |
| 01 | [01-threat-model.md](01-threat-model.md) | Competitive matrix, threat model, attack trees |
| 02 | [02-architecture.md](02-architecture.md) | Zero-trust architecture, codebase simplification, risk gate, module specs, state-machine security, formal invariants, state reconciliation engine |
| 03 | [03-halt-and-risk.md](03-halt-and-risk.md) | Network halt system L0–L7, systemic risk engine, circuit breakers, recovery |
| 04 | [04-crypto-consensus.md](04-crypto-consensus.md) | Cryptographic stack, TSS infrastructure, vault architecture, post-quantum strategy, consensus design, performance targets |
| 05 | [05-economics-operations.md](05-economics-operations.md) | Economic security, upgrade, operations, monitoring, incident response, formal verification plan, remaining research |

Red-team findings that describe live attack paths against a public pre-mainnet
repository are **not** in this directory. They are in the private working file
`.claude/plans/protocol-redteam-2026-11-08.md`; [01](01-threat-model.md) lists
their classes only.

## Executive summary

Maya2C already has the hard parts most chains in the matrix lack together:
post-quantum transaction and validator signatures (ML-DSA + SLH-DSA hybrid,
invariants 29–31), a deterministic invariant guard with per-module breakers that
never stop a transfer (invariant 28), forward-only recovery rules written before
any incident (ADR-077), a genesis-committed upgrade and suite-sunset schedule
(ADR-082, invariant 34), 256-bit money everywhere (ADR-066), and DAG-BFT
finality (ADR-027).

What it does not have yet is listed plainly, because that list *is* the plan:

1. **Throughput is not where it must be.** MEASURED: 502.2 finalized transfers/s at
   commit 23a158f4 (`reports/bench/tps-1793027291.md`), and **31.3/s with 2,987 of
   6,000 accepted transactions never finalized** at 6029a4ba
   (`reports/bench/tps-1794112649.md`, shared machine). The second run is a P0
   question before anything in this document: a regression, or load from the
   other processes, must be decided by a re-run on an idle machine.
2. **Certificate bandwidth caps validator count.** COMPUTED in [04](04-crypto-consensus.md) §5:
   ML-DSA-87 votes make a full certificate ≈310 KB at `MAX_COMMITTEE = 100`.
   Lazy certificates cut that ≈150×; a consensus change, post-mainnet.
3. **No cross-chain settlement exists yet** (ADR-065 option 2 is at `u64::MAX`).
   Vault, TSS and chain-isolation designs here are the specification it is built to.
4. **The halt system is half built.** L1 (module breaker) is REAL; L0 and L2–L7 are
   specified in ADR-077/MIP-0001 and here, and ship post-mainnet at `u64::MAX`.
5. **No external review exists.** Owner decision 2026-10-28: none until after
   mainnet. Every score below therefore stops short of 10 on that one check.

Design stance, in one line each:

- **Trust nothing, prove transitions:** every state change is a pure function of
  (committed state, block); the guard re-checks conservation on every block.
- **Complexity is a budget:** a CI gate (`cargo xtask risk-gate`, [02](02-architecture.md) §3)
  refuses a PR that raises a module's risk score without a written reason.
- **Degrade, never collapse:** eight halt levels, scoped to cells; the core ledger
  is never frozen and small transfers never stop.
- **AI proposes, rules act:** ML detectors are advisory; only integer rules and
  stake signatures change state (invariant 20, ADR-077 D1).
- **Price-independent security:** caps and throttles are in units the chain can
  count without an oracle; a 95% price drop changes no on-chain rule
  ([05](05-economics-operations.md) §1).

## Scoring method

The brief asks for six scores per subsystem and a redesign below 9.8/10. A score
nobody can recompute is an invented number (Proof discipline 5), so a score here
is **the fraction of ten evidence checks that pass, times ten**. 9.8 therefore
means all ten pass. Checks per dimension:

| Dimension | Ten checks (each pass/fail, each cites a file) |
|---|---|
| Security | threat model written · numbered invariant pinned by a test · exploit-replay test · fuzz target · formal proof (Kani/Lean/Z3) of the core rule · no `unsafe` (or SAFETY + test) · secrets typed + zeroized · red-team attempt recorded as a test · runtime guard covers it · **external review** |
| Complexity | functions <50 lines · file <800 lines · nesting ≤4 · cyclomatic ≤15 per fn · ≤ allowed deps (forbidden matrix) · no cycle · one owner of each state prefix · no feature-gated consensus · public API ≤ budget · dead code 0 |
| Scalability | throughput measured · latency measured · state growth bounded by rule · bandwidth computed per validator · scales with cores (measured) · storage pruning verified · no O(n²) on hot path · bounded allocations in loop · committee size limit justified · load test on separate machines |
| Maintainability | doc coverage ≥90% · ADR for every decision · spec section exists · conformance vectors · lint debt not rising · tests run <5 min · single crate owner · comments say why · reproducible build · upgrade path documented |
| Auditability | spec ↔ code mapping · invariant ↔ test mapping · features.toml entry with test · deterministic replay tool · event log for every state change · SBOM · cargo-vet coverage · provenance attestation · audit checklist · **external review** |
| Resilience | rejoin after outage tested · crash-consistency test · halt + restart rehearsed · breaker scoped to module · recovery runbook · chaos/attacknet run · rollback impossible by rule · backup/restore drill · alerting wired · degraded-mode tested |

`cargo xtask scorecard` (to build, [02](02-architecture.md) §3) computes this from the
tree. Until it exists, the scorecard below is a **design target** column plus the
checks *known* to fail today, not a computed current score.

## Scorecard (design target after this TDD; known failing checks today)

| Subsystem | Target (all 6 dims) | Checks known to fail today |
|---|---|---|
| Consensus (DAG-BFT) | 10 | external review; throughput regression unexplained; load test on separate machines; lazy certificates not built |
| Execution / state | 10 | external review; parallel execution RESEARCH; bounded allocation in apply loop (Execution Directive 3) |
| Cryptography (PQ suites) | 10 | external review; PQ threshold signing is open research (§04-3) |
| Invariant guard / breakers (L1) | 10 | external review; L0, L2–L7 not built |
| Halt system L0–L7 | 10 | not built (post-mainnet, `u64::MAX`) |
| Risk engine | 10 | not built |
| Cross-chain / vaults / TSS | 10 | not built (ADR-065 option 2 at `u64::MAX`) |
| Economics | 10 | insurance reserve and progressive exit throttle not built; external economic review |
| Upgrades (ADR-082) | 10 | two-binary late-node rehearsal not run in one process |
| Operations / monitoring | 10 | single-machine launch disclosed (ADR-042); no independent operator |
| Tooling (risk-gate, scorecard) | 10 | not built |

**Redesign rule applied.** Every subsystem is below 9.8 today. This TDD redesigns
each one to a form where all ten checks *can* pass; the remaining gap is work and
one external review, which the owner has deferred until after mainnet.

## Build order (band-ordered, CLAUDE.md)

| Step | What | Band | When |
|---|---|---|---|
| 0 | Re-run `bench-tps` on an idle machine; explain 31.3/s vs 502.2/s | P0 | now |
| 1 | `cargo xtask risk-gate` + `scorecard` (CI only, touches no consensus) | P2 | now |
| 2 | Genesis-permanent checks: halt-level and risk records fit under existing prefixes or `up:` feature gates, so no re-genesis later | P3 | before 12-03 freeze |
| 3 | L0 sentinels + alert feed (off-chain, advisory) | P5 | post-mainnet |
| 4 | L2–L5 cells, DMZ gateway, progressive throttles (`u64::MAX`) | P5 | post-mainnet |
| 5 | Risk engine (integer, in-state) | P5 | post-mainnet |
| 6 | Lazy certificates, committee scaling | P5 | post-mainnet, ADR + Eric |
| 7 | Cross-chain vaults: FROST/ROAST + DKLs23 custody | P5 | post-mainnet, ADR-065 decision |
| 8 | L6–L7, recovery transactions | P5 | post-mainnet |

Step 2 is the only part that touches the launch. ADR-082 already lets every later
step arrive as a scheduled feature without a re-genesis; step 2 verifies that claim
for each new record type before the freeze.
