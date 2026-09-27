# 20 — Mainnet readiness

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 20

**Verdict: NO-GO.** Ten gates fail on evidence, and two need a person. The
decision to launch belongs to the project owner, gate by gate, with
"APPROVED: <step>". Nothing here is published, deployed or announced.

> DONE WHEN: audit packets exist for every scope; the findings tracker is in
> place; `cargo xtask go-no-go` runs and reports honestly; genesis rehearsal
> procedure and launch sequence are written; ROADMAP.md and this report exist.

| Condition | Where |
|---|---|
| Audit packets, four scopes | `docs/audit/README.md`, `docs/audit/KNOWN_ISSUES.md` |
| Findings tracker | `docs/audit/FINDINGS.md`. An empty table is a NO-GO: no findings means no audit |
| `cargo xtask go-no-go` | below |
| Genesis rehearsal procedure, launch sequence, first 90 days | `docs/GENESIS.md` |
| Roadmap | `docs/ROADMAP.md` |

## Go/no-go, as computed

```
$ cargo xtask go-no-go
PASS         Reality ledger: every claim names a real test
             evidence: features.toml (cargo xtask coverage)
FAIL         Spec: no consensus rule without vectors
             7 consensus rules without full vector coverage
FAIL         SLOs: metric, dashboard, alert, runbook for each
             6 SLOs lack a metric, dashboard panel, alert or runbook
FAIL         DAG-BFT wired into the node
             the node does not depend on dag-bft (ADR-015)
FAIL         Staking and slashing built
             no staking subsystem in features.toml
FAIL         Fee market active
             the node does not link maya-fee-market outside tests: no fee is charged on any network
FAIL         External audits: at least one, zero open critical/high
             no external audit has reported: the tracker has no findings, which is not the same as zero
PASS         State sync rehearsed in the last 30 days
             evidence: reports/14-scale.md (100M-account state sync, 0 days old)
PASS         Crash restore rehearsed in the last 30 days
             evidence: reports/12-performance.md (1,000 kill -9 restarts, 0 days old)
PASS         Coordinated restart rehearsed in the last 30 days
             evidence: reports/19-operations.md (12-node coordinated restart, 0 days old)
FAIL         Attacknet ran ≥ 4 weeks without an unresolved halt or fork
             no evidence of an attacknet ran (reports/attacknet.md does not exist)
FAIL         Incentivized testnet met SLOs ≥ 4 consecutive weeks
             no evidence of an incentivized testnet ran (reports/incentivized-testnet.md does not exist)
FAIL         External validators across enough providers and countries
             no evidence of an external validator set exists (reports/validator-diversity.md does not exist)
FAIL         Genesis parameters frozen in a signed file
             no evidence of genesis is frozen and signed (genesis/mainnet.json.sig does not exist)
NEEDS HUMAN  Incident-response on-call rota staffed
             confirm the rota in docs/security/INCIDENT_RESPONSE.md is staffed
NEEDS HUMAN  Legal sign-off received
             the project owner confirms this manually

4 PASS, 10 FAIL, 2 NEEDS HUMAN — NO-GO
```

The tool computes every gate from files and code. It never reads the State
column of `LAUNCH.md`, and it never prints PASS without an evidence path.

**The fee-market gate, corrected.** Its first version keyed on the existence
of `FeeConfig::DISABLED`, which would have stayed true after activation. The
node in fact links `maya-fee-market` only as a dev-dependency, so the gate
now tests for the real dependency.

## Known risks, in plain language

| Risk | What could happen | Mitigation | Owner (role) |
|---|---|---|---|
| The chain has no BFT finality yet | deposits are only probabilistically final; a hash-rate renter can reorganise | wire DAG-BFT (ROADMAP 1); until then, confirmation tables in `docs/integrations/EXCHANGES.md` | consensus lead |
| No staking, no slashing | nothing economic stops a validator misbehaving | build it (ROADMAP 2); the signer already refuses to double-sign | protocol lead |
| Post-quantum weight | 13 KB per transfer; 10 Gbit/s and 11 TB/day at 10k TPS | key hash on the wire, signature pruning (not built), a throughput choice made from `reports/13-pq-weight.md` | protocol economist |
| Validator cap from certificate size | above ~100 validators, certificates alone exceed a 1 Gbit/s NIC at 1 s rounds | ADR-021: STARK-aggregated certificates before raising the cap | consensus lead |
| Draft economics fail two scenarios | an 80 % price drop leaves 30 validators; 100x usage burns 71 % of supply | revisit the burn share and add a validator income floor before activating fees | protocol economist |
| Self-written signer AKE | an authentication flaw would let an impostor request signatures | external review, audit scope (a) | security lead |
| Unaudited shielded pool | undetectable minting if the AIR is under-constrained | dark until audited; the ceremony tool refuses value-bearing chains | cryptography lead |
| Operations tooling unproven on a real network | unknown failure modes at scale | staging network (plan, awaiting approval), attacknet, incentivized testnet | SRE lead |

## What is not yet proven

- That the node reaches consensus with other nodes under a BFT rule. It
  does not run one.
- Any end-to-end TPS, finality or RPC figure on more than one machine.
- That the fee market's parameters survive real traffic. They are tuned on
  synthetic load only.
- That any HSM or KMS can hold a hybrid key for the signer.
- That the cryptography resists side channels beyond the dudect runs so far.
  No external review has happened.
- That the formal proofs cover the real code. They cover models of it
  (Lean), bounded integer encodings (Z3), and Kani harnesses that were not
  run this session.

## Re-run 2026-09-27

After DAG-BFT, staking, the live fee market and the security council landed:

```
$ cargo xtask go-no-go
PASS         DAG-BFT wired into the node
NEEDS HUMAN  Staking and slashing built   (features.toml: staking-and-slashing, verified)
NEEDS HUMAN  Fee market active            (on any genesis with bft.fees; ADR-029)
FAIL         External audits … / Attacknet ≥ 4 weeks / Incentivized testnet ≥ 4 weeks /
             External validators / Genesis frozen in a signed file
6 PASS, 6 FAIL, 4 NEEDS HUMAN — NO-GO
```

Every remaining FAIL is an act outside this repository: an external audit,
weeks of a public attacknet and incentivized testnet, outside operators, and
the owner signing a mainnet genesis. The gate reports them honestly, which is
what the DONE WHEN asks of it.
