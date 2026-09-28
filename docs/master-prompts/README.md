# Master Prompts 1–30: coverage ledger

The thirty briefs live in `Prompts/` (three files, 1–10, 11–20, 21–30). This
file records, for each one, whether its **DONE WHEN** is met and the largest
gap. It is the answer to "is prompt N finished", and it is deliberately
separate from `features.toml`.

**The handover** — the release build, its evidence, and what only the owner
can do next — is [`reports/31-handover.md`](../../reports/31-handover.md).

## Why separate from `features.toml`

`features.toml`'s 164 `[[feature]]` rows (`P001`…`P164`) belong to the
*earlier* 164-prompt trajectory (`docs/trajectory.md`), not to these thirty
master prompts. The ids collide — `P023` there is "Prompt 23 —
Post-Quantum Cryptography", not Master Prompt 23 — and the 2026-09-26 audit
misread that collision as eight false `verified` claims. They are not false
claims about these prompts; they are a different numbering. This ledger uses
`MP01`…`MP30` so the two cannot be confused again.

## How the status was established

2026-09-26: four read-only audits, one per range (3–10, 11–16, 17–22,
23–30), each told to count a requirement satisfied only with a `path:line`
for the code *and* a test or report exercising it. Their counts are below.
They are audit-level evidence, not re-derived here, and one known error is
corrected in the notes (the explorer exists as `apps/explorer`, crate
`maya-explorer`). MP01–MP02 were verified directly in the session that built
them; their measured gates are in `reports/01-foundation.md` and
`reports/02-crypto.md`.

**DONE WHEN met** means every criterion in the brief's DONE WHEN line is met
on evidence. "Partial" means some are.

## Ledger

| MP | Title | Sat / Part / Miss | DONE WHEN | Largest gap |
|---|---|---|---|---|
| 01 | Foundation | — | **Met** | — (`reports/01-foundation.md`) |
| 02 | Cryptography, ZK, custody | — | **Met, with findings** | Every section built and tested (`reports/02-crypto.md`). What stays open is outside the tree: HQC decapsulation in `hqc-kem 0.1.0-rc.0` leaks (|t| 47.5, n = 176k), so HQC stays draft and never sole; the shielded circuit awaits an external audit; threshold custody signs off chain only until a peer-reviewed threshold ML-DSA exists |
| 03 | State, storage, economics | — | **Met** | Fee market live on any genesis with `bft.fees` (ADR-029, 2026-09-27); recursive-compaction folding not started (not in DONE WHEN) — `reports/03-state.md` §7 |
| 04 | Consensus, mining, scaling | — | **Met** | Node runs DAG-BFT (ADR-027); 990 tx/s finalized, 4 validators in one process (perf profile); finality mean 0.55 s on a 5-process devnet; GPU parity 7/7 on an RTX 5070 — `reports/04-consensus.md` §6 |
| 05 | VM, contracts, on-chain AI | — | **Met** | VM + differential gas tests pass; multi-VM Phase A (revm + solana-sbpf, unified mapping, measured gas conversion); zkML verify 1.52 ms on a transparent STARK (linear model). Finding: Pulley tier fuel diverges on Windows — tier stays off — `reports/05-vm.md` §6 |
| 06 | DeFi, finance, identity, physical | — | Partial | CBDC vault with ZK-KYC, dark pool (commit-reveal, 1,000 orders 0.83 ms), US/UK/DE tax and the 500-transaction compliance test built; **energy protocol parsers** (Modbus/TCP, IEC 61850 GOOSE, IEEE 1547 trip rules; SIM grid) and **biometric PoP** (fuzzy commitment → ML-DSA-65, 100,000 verifications in 54.5 s; no liveness or uniqueness). **dark pool over secret shares** with **SPDZ MACs catching dishonest servers**; **Bitcoin lock/mint bridge** (six-block reorg test, real mainnet parsing); **EEG/BCI SIM** (EDF parser, SSVEP liveness, 3 s proofs). **robot swarms** (MAVLink/ROS 2 CDR vs reference implementations, 100 agents); **10,000-device barter** with SLA slashing; energy scale tests. **satellite NDVI oracle** on real Sentinel-2 pixels (3 satellites, median, dispute window, parametric payout). Dealer-free MPC offline phase not built — `reports/06-finance.md` |
| 07 | Networking, transports, time | — | **Met**, eBPF load skipped with a stated reason | XDP load needs `CAP_NET_ADMIN`; runs in `ebpf-net.yml` — `reports/07-network.md` |
| 08 | Security, formal verification | — | **Met** (report complete; invariants proven or listed open) | Line coverage not measured (no llvm-cov); Kani not run this session — `reports/08-security.md` |
| 09 | Governance, wallets, explorer, SDKs | — | Partial | SDK e2e **met for TypeScript** against a live node (found 2 gateway bugs); Go/Python SDKs are placeholders; wallet GUI WebDriver suite never run — `reports/09-product.md` |
| 10 | Infrastructure, release, genesis | — | **Met** | `deploy-production.sh --target local-k3d` end to end on real k3d, 12/12 Ready and answering RPC (2026-09-27) — `reports/10-launch.md` §0 |
| 11 | Production baseline, scope freeze | — | **Met** | Full run 2,626 passed / 0 failed; gap register, release check — `reports/11-reality-audit.md`, `reports/11-gap-register.md` |
| 12 | Execution performance | — | Partial | Verify-once + parallel verification; block preview reused on insert (44/44, +4%); PGO measured, no gain; bottlenecks re-measured (PQ verification dominates; root 0.48 µs/account, incremental tree needed before large state). Needs: a dedicated benchmark runner for the CI regression gate (owner) — `reports/12-performance.md` |
| 13 | PQ weight: signatures, bandwidth, DA | — | **Met** | Bytes before/after, bandwidth per TPS level, storage/day with hardware; key registration and rotation tests pass (smart-account, 10/10); ADR-021; DA withholding passes; verify-once cache built. Caveat: the key-hash frame is not in the node's own tx format — `reports/13-pq-weight.md` §9 |
| 14 | State sync, light clients, sharding, RPC | — | **Met** | NODE_TYPES measured; 100M-account sync 9.4 s (SIM); lying-RPC test passes; RPC curve; validator-count limit **measured** with real engines (n=100: 0.84 cores, 384 Mbit/s per validator at 1 s rounds; bandwidth-bound) — `reports/14-scale.md` §5a |
| 15 | Protocol spec, conformance, upgrades | — | **Met** | 44 rules, 71 vectors, **0 consensus gaps** (CON-4…8, ROOT-5, TX-4 added 2026-09-28); the node agrees on every vector, the TS verifier on the five families it reads (not `consensus.json`); CI job not yet observed on GitHub — `reports/15-spec.md` |
| 16 | Validator and key security | — | **Met** | Remote signer + ADR-022 (why no HSM/KMS yet); slashing protection passes; DoS sims pass; incident-response and security-council pause rehearsals recorded (2026-09-27) — `reports/16-validator-security.md` §5–6 |
| 17 | Integration layer | — | Partial | Mesh `check:data` **passes**; guide commands checked in CI; dev path measured on Windows/Linux (~3 s with binaries, 7–10 min from source, so 5 min not met from source). Needs: WalletConnect Cloud project id (owner), a Mac; `check:construction` blocked by Mesh having no PQ signature type — `reports/17-integrations.md` |
| 18 | Economic security, launch economics | — | **Met** | 9 scenarios × 730 days; cost-of-attack tables; fee parameters derived from node measurements (5,377 B/transfer, 990 tx/s/node, ~1 block/s → 2.5 MiB target); treasury/vesting tests; legal questions exist (the legal review itself is external) — `reports/18-economics.md` §3a |
| 19 | Operations, public testnet | — | **Met** (engineering) | Operator install/restore/upgrade on k3d; slo-check 7/7; 20/20 runbooks rehearsed; coordinated restart recorded; testnet plans and infracost await the owner's approvals — `reports/19-operations.md` |
| 20 | Mainnet readiness | — | **Met** (verdict NO-GO) | `go-no-go` re-run: 6 PASS, 6 FAIL (all external: audits, attacknet, incentivized testnet, outside validators, signed genesis), 4 NEEDS HUMAN — `reports/20-mainnet-readiness.md` |
| 21 | Weakness map, beat bars | — | **Met** | Dated WEAKNESS_MAP and BEAT_BARS; prior-art files exist for every headline feature (7 still say "partially searched", which blocks any superlative); harness runs Ethereum end to end via anvil; ADR-016 updated — `reports/21-strategy.md` |
| 22 | Accounts without pain | — | Partial | 20-person usability study needs participants — `reports/22-accounts.md` |
| 23 | Safe-by-default contracts | — | Met (VM, opt-in) | Resource and capability rules enforced by the VM host through the opt-in `maya_res` imports: 9 VM tests with real WASM, full revert on fault/trap/out-of-gas/invariant. Not on the consensus import surface; templates have listed open properties, not proofs — `reports/23-contract-safety.md` |
| 24 | Developer platform | — | Partial | `maya2c` dev (1.9 s to first block), fork, replay (state root reproduced) and time-travel debugger **work end to end**; **debugger in VS Code over DAP** (`maya2c dap`, step back exact); host-call granularity, eager copies; developer study needs participants — `reports/24-devx.md` |
| 25 | Interop without trusted bridges | — | Partial | Beacon light client verifies **real mainnet finality** (505/512 signers); **intent settled across two real devnets** by hash-locked swap (8.3 s). Not ZK — `reports/25-interop.md` |
| 26 | Scale without fragmentation | — | **Met** | Split-validator prototype measured (verification in worker processes: ×5.14 at 12 workers, 36,530 tx/s); local fee markets and lanes pass; viral-app sim keeps others within SLO. Caveat: processes on one host, not across a network — `reports/26-scale.md` |
| 27 | Privacy primitive, compliance | — | Partial | Private contract state (STARK-proved commitment updates, VM `maya_priv`) and association-set inclusion **built and reviewed**; viewing keys, exclusion proofs, limits doc done. Needs: a phone for mobile proving; external audit before activation — `reports/27-privacy.md` |
| 28 | Quantum-safe harbor | — | Partial | Exposure tool on real BTC **and** ETH data; **PQ vaults end to end on a devnet** (ADR-030, invariant 32, honest risk label). **archival re-sealing** scheduled by suite deprecation and **reconciled custodian statements** (`maya2c custody-report`). No outside-asset bridge into vaults (no MP25 route for BTC/ETH); the node's archive task does not seal yet — `reports/28-quantum-harbor.md` |
| 29 | Wallet, explorer, portal UI | — | Partial | Design system + screenshot diffs in CI; no usability study, no device budgets, wallet e2e never run — `reports/29-interface.md` |
| 30 | Adoption engine, public proof | — | Partial | Caller identity live (ADR-026 accepted); NFT game runs through the node with owner checks. No public testnet; outside-developer port times and approvals pending — `reports/30-adoption.md` |

**Updated 2026-09-26 at the end of the `claude/task-0g86kl` run: six met
(01, 02, 07, 08, 11, 15), one met in substance (10), twenty-one partial, one not met
(19), and MP20's artefacts complete with a computed NO-GO.** The
`Sat / Part / Miss` counts of the earlier audit are superseded by each
report's own DONE WHEN table and are not re-derived here.

## Requirements no amount of code satisfies

Some DONE WHEN criteria need people or money outside this repository. They
are listed so they are not mistaken for engineering left undone:

- An **external cryptography audit** of the shielded circuit (MP02, MP20, MP27).
- **Independent security audits** and a bug-bounty programme (MP20).
- A **developer study with ten or more developers** (MP24), and public
  adoption figures (MP30).
- A **public testnet** with outside operators (MP19), and anything that deploys
  to live infrastructure — CLAUDE.md Standing Order 6: nothing that costs money
  or touches a live server runs without being asked first.
- **Legal review** of launch economics (MP18).

The code, harnesses, runbooks and reports that *prepare* each of these are
engineering and are in scope; the external act itself is not.
