---
title: 'Testnet programme — plans, awaiting approval'
editUrl: false
# GENERATED from docs/TESTNET_PROGRAM.md by scripts/ingest.mjs. Edit the source, not this.
---
Master Prompt 19 §4-5. **Nothing here has been launched, provisioned or
announced.** Each phase starts only after an explicit
"APPROVED: <phase name>". An infracost estimate is required before each
approval request, and no estimate exists yet (`reports/19-operations.md`).

## Staging network (plan)

| Item | Plan |
|---|---|
| Nodes | 50 full nodes: 40 validators (once DAG-BFT is wired) + 10 RPC |
| Regions / providers | at least 3 regions on 2 providers (the terraform in `infra/terraform` targets one provider today, so a second is to be written) |
| Size per node | `docs/NODE_TYPES.md` validator row: 4 vCPU, 8 GiB, NVMe, 1 Gbit/s |
| Purpose | every release candidate soaks here before any public release; every SLO in `docs/SLO.md` is measured here |
| Approval | "APPROVED: staging-network" |

## Phase 1 — public testnet

Open node running, faucet (`apps/faucet`), explorer (`apps/explorer`), status
page. Exit criteria: 30 days without a chain halt; `cargo xtask slo-check`
green; the conformance suite green on every release. Approval:
"APPROVED: public-testnet".

## Phase 2 — attacknet

Invited researchers get validator slots and a bounty for halting, forking or
censoring. Rules of engagement:

- in scope: the chain, its p2p and RPC surfaces, the signer channel;
- out of scope: third-party infrastructure, social engineering, anything
  touching real funds;
- disclosure through `SECURITY.md`.

Publishing the bounty needs "APPROVED: bug-bounty"; running the attacknet
needs "APPROVED: attacknet".

## Phase 3 — incentivized testnet

External validators across many countries and providers, with uptime and
performance tracked. The rewards policy is drafted for legal review
(`docs/legal/QUESTIONS_FOR_COUNSEL.md`: token classification of testnet
rewards). Approval: "APPROVED: incentivized-testnet", after counsel.

## Load events

Scheduled public stress tests with `maya2c-loadgen` and `maya2c-rpc-load` at
increasing levels. Results are published with the methodology in
`docs/BENCHMARK_METHODOLOGY.md` so anyone can reproduce them.
