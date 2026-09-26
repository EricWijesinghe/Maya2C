# Master Prompts 1–30: coverage ledger

The thirty briefs live in `Prompts/` (three files, 1–10, 11–20, 21–30). This
file records, for each one, whether its **DONE WHEN** is met and the largest
gap. It is the answer to "is prompt N finished", and it is deliberately
separate from `features.toml`.

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
| 03 | State, storage, economics | 6 / 3 / 2 | Not met | DNA archive codec absent; recursive history compaction absent; no `reports/03-state.md` |
| 04 | Consensus, mining, scaling | 5 / 4 / 0 | Not met | No `ConsensusMode` / consensus ADR; DAG (`blockgraph`) not called by consensus; no `reports/04-consensus.md` |
| 05 | VM, contracts, on-chain AI | 4 / 2 / 0 | Partial | Pulley tier, differential gas tests, revm/SBF; no `reports/05-vm.md` |
| 06 | DeFi, finance, identity, physical | 6 / 2 / 0 | Not met | Compliance ZK proofs, macro-shock simulator, SPV 6-block reorg test; no `reports/06-finance.md` |
| 07 | Networking, transports, time | 5 / 0 / 0 | Partial | FSO / acoustic / OAM SIM models, timing service; no `reports/07-network.md` |
| 08 | Security, formal verification | 5 / 2 / 1 | Not met | No Lean/Aeneas pipeline, no Z3 symbolic execution, no `THREAT_MODEL.md` / `SECURITY.md` |
| 09 | Governance, wallets, explorer, SDKs | 7 / 1 / 1 | Partial | Animated-QR PQ signature export, passkey unlock, uniffi / TypeScript SDKs |
| 10 | Infrastructure, release, genesis | 1 / 2 / 4 | Not met | `deploy-production.sh`, `configure_environment.sh`, `LAUNCH.md`, pre-ignition audit, release workflow |
| 11 | Production baseline, scope freeze | 0 / 3 / 9 | Not met | ADR-launch-scope; production standing orders; `crates/loadgen`; `READINESS.md` |
| 12 | Execution performance | 0 / 7 / 15 | Not met | Loadgen, parallel-execution ADR, crash-consistency (1k `kill -9`), 100k differential blocks |
| 13 | PQ weight: signatures, bandwidth, DA | 0 / 5 / 11 | Not met | Certificate aggregation bench + ADR, 2D RS data availability, withholding test |
| 14 | State sync, light clients, sharding, RPC | 0 / 5 / 8 | Not met | `NODE_TYPES.md`, 100M-account sync bench, RPC scaling curve |
| 15 | Protocol spec, conformance, upgrades | 0 / 4 / 13 | Not met | No `spec/`, no conformance vectors, no independent verifier, no upgrade rehearsal |
| 16 | Validator and key security | 0 / 6 / 12 | Not met | `bins/maya2c-signer`, slashing-protection DB, DoS sim scenarios |
| 17 | Integration layer | 0 / 3 / 23 | Not met | Mesh (Rosetta) API; exchange / custodian integration docs |
| 18 | Economic security, launch economics | 0 / 1 / 19 | Not met | Economic simulator (`crates/econ-sim`) |
| 19 | Operations, public testnet | 0 / 2 / 20 | Not met | `docs/SLO.md`, runbooks, k8s operator, 12-node k3d automation |
| 20 | Mainnet readiness | 0 / 0 / 24 | Not met | Audit packets, `cargo xtask go-no-go`, genesis plan |
| 21 | Weakness map, beat bars | 0 / 0 / 13 | Not met | `WEAKNESS_MAP.md`, `BEAT_BARS.md` with sourced prior art |
| 22 | Accounts without pain | 0 / 1 / 21 | Not met | Smart-account validation, guardian recovery, intent display |
| 23 | Safe-by-default contracts | 0 / 3 / 9 | Not met | Contract SDK with resource types and capabilities; `bins/maya2c-cli` is empty |
| 24 | Developer platform | 0 / 2 / 10 | Not met | CLI (`dev`, `fork`, `replay`, debugger); SDKs from one definition |
| 25 | Interop without trusted bridges | 0 / 0 / 10 | Not met | ZK light clients of other chains; trust-assumptions table |
| 26 | Scale without fragmentation | 0 / 0 / 7 | Not met | Everything |
| 27 | Privacy primitive, compliance | 0 / 2 / 8 | Not met | Shielded pool blocked on an external circuit audit (`CIRCUIT_IS_AUDITED = false`) |
| 28 | Quantum-safe harbor | 0 / 2 / 7 | Not met | Migration service for other chains' assets |
| 29 | Wallet, explorer, portal UI | 0 / 2 / 12 | Not met | Shared design system across the three apps |
| 30 | Adoption engine, public proof | 0 / 1 / 11 | Not met | Five reference applications |

**Two prompts met, three partial, twenty-five not met.**

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
