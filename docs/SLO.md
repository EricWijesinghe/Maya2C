# Service level objectives — mainnet core

Master Prompt 11 §6. **Every number below is a TARGET** until Master Prompt
19 measures it on a staging network and replaces it with a measured value or
a named gap. A target is a design goal, not a claim.

Each SLO has four companions that `cargo xtask slo-check` requires: a
**metric** the node exports, a **dashboard** panel, an **alert**, and a
**runbook**.

| ID | Objective | Target | Metric | Dashboard | Alert | Runbook |
|---|---|---|---|---|---|---|
| finality-p50 | Finality latency, median | TARGET ≤ 2 s | `maya_finality_latency_seconds` | infra/grafana/dashboards/slo.json#finality | infra/grafana/alerts/slo.yaml#FinalitySlow | docs/runbooks/fell-behind.md |
| finality-p99 | Finality latency, 99th percentile | TARGET ≤ 6 s | `maya_finality_latency_seconds` | infra/grafana/dashboards/slo.json#finality | infra/grafana/alerts/slo.yaml#FinalitySlowP99 | docs/runbooks/fell-behind.md |
| liveness | Blocks produced per expected slot over 1 h | TARGET ≥ 99.9% | `maya_blocks_produced_total` | infra/grafana/dashboards/slo.json#liveness | infra/grafana/alerts/slo.yaml#ProductionStalled | docs/runbooks/chain-halt.md |
| rpc-availability | Public RPC success ratio over 30 d | TARGET ≥ 99.9% | `maya_rpc_requests_total` | infra/grafana/dashboards/slo.json#rpc | infra/grafana/alerts/slo.yaml#RpcErrors | docs/runbooks/rpc-overload.md |
| rpc-latency | Public RPC p99 for `get_balance` | TARGET ≤ 250 ms | `maya_rpc_duration_seconds` | infra/grafana/dashboards/slo.json#rpc | infra/grafana/alerts/slo.yaml#RpcSlow | docs/runbooks/rpc-overload.md |
| state-sync | New full node: empty disk to head | TARGET ≤ 2 h at 100M accounts | `maya_sync_duration_seconds` | infra/grafana/dashboards/slo.json#sync | infra/grafana/alerts/slo.yaml#SyncSlow | docs/runbooks/snapshot-corrupt.md |
| validator-downtime | Downtime before a validator is slashed | TARGET: slashed only after 10,000 missed consecutive rounds (~5.5 h at 2 s) | `maya_validator_missed_rounds` | infra/grafana/dashboards/slo.json#validators | infra/grafana/alerts/slo.yaml#ValidatorMissing | docs/runbooks/signer-unreachable.md |

## Error budgets

A 99.9% objective over 30 days is a budget of 43 minutes. When a budget is
spent, releases that are not fixes stop until the next window (Master Prompt
19 §2).

## What is measured today

Nothing on a multi-region network — no staging network exists. What the
simulator shows about the DAG-BFT engine (ordering rounds of ~150 ms on a
modelled 20–80 ms link, `reports/04-consensus.md`) is an input to these
targets, not a measurement of them.
