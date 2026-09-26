---
title: 'Roadmap after launch scope'
editUrl: false
# GENERATED from docs/ROADMAP.md by scripts/ingest.mjs. Edit the source, not this.
---
Master Prompt 20 §6. Order and evidence come from ADR-016. Nothing activates
by binary version: each module activates through governance at a
written-down height, after its audit.

## Before any mainnet (blocking, from ADR-016)

| # | Work | Evidence that it is done | Audit |
|---|---|---|---|
| 1 | Wire DAG-BFT into the node (ADR-015) | `cargo xtask go-no-go` passes "DAG-BFT wired"; conformance vectors for the commit rule | (b) consensus |
| 2 | Staking and slashing | spec section + vectors; slashing tests with remote-signer evidence | (c) state machine |
| 3 | Activate the fee market | the node links `maya-fee-market`, with a finite activation height; econ findings on burn share and validator floor addressed (`reports/18-economics.md`) | (c) |
| 4 | Verified-signature cache (93 % of apply time) | `apply_pipeline` before/after | (b) |
| 5 | RPC, finality, sync and missed-round metrics | `cargo xtask slo-check` green | (d) operations |

## Deferred modules, in activation order

| Order | Module | Evidence that brings it in | Audit |
|---|---|---|---|
| 1 | Shielded pool | external audit of the joinsplit AIR (`CIRCUIT_IS_AUDITED`) | (a) cryptography |
| 2 | Smart accounts (key hash on the wire) | wired into the transaction format; conformance vectors | (c) |
| 3 | DeFi / RWA / identity modules | per-module spec, invariant hooks, load tests | (c), per module |
| 4 | Neural gas | trained on testnet traffic and beating plain EIP-1559 in `econ` | economics + determinism |
| 5 | Threshold-lattice custody | peer review of the dealerless keygen (ADR-014) | (a) |
| 6 | HQC second KEM | constant-time implementation (ADR-009) | (a) |
| 7 | Multi-VM (EVM) | gas-parity suite; classical-security labelling (ADR-023) | VM + EVM |
| — | zkML, PoUW, exotic transports, frontier HAL | the evidence ADR-016 names; none is scheduled | — |

## Beyond the protocol

- **Second client**: close the spec gaps first (VM, extra state layers,
  consensus; `docs/SECOND_CLIENT.md`), then fund by treasury grant
  (`crates/treasury` staged approval).
- **Cryptographic watch**: `docs/CRYPTO_WATCH.md`, reviewed quarterly.
- **Scaling milestones tied to measured limits** (`reports/14-scale.md`,
  ADR-021):
  - raise the validator cap above 100 only with STARK-aggregated
    certificates that meet ADR-021's acceptance test;
  - raise the block target only with a measured capacity review.
