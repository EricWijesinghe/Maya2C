# 19 — Operations at scale and the testnet programme

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 19

> DONE WHEN: Kubernetes operator handles install, snapshot restore and
> rolling upgrade on the local cluster; slo-check passes; the 20 runbooks
> exist and are rehearsed; coordinated-restart rehearsal is recorded;
> testnet phase plans and infracost estimates are here, waiting for approvals.

| Condition | Result |
|---|---|
| Kubernetes operator | **not built.** No Kubernetes cluster can run in this sandbox (`reports/10-launch.md`: k3d blocked by egress, nested runc refused), so an operator could not be tested |
| `cargo xtask slo-check` passes | **yes** (2026-09-27): 7 of 7. The node now registers and records finality latency, RPC requests and duration (middleware), sync duration and per-validator missed slots; a devnet scrape shows real values (§1a) |
| 20 runbooks exist | **yes**, `docs/runbooks/` |
| …and are rehearsed | **11 of 20** have a test or measured run behind them (index below); 9 do not |
| Coordinated-restart rehearsal | **recorded** (§3) |
| Testnet phase plans | **written**, `docs/TESTNET_PROGRAM.md` |
| infracost estimates | **none.** `infracost` is not installed and needs a pricing API key; **no approval is requested** |

## 1a. slo-check, after the metrics landed (2026-09-27)

```
$ cargo xtask slo-check
ok    finality-p50
ok    finality-p99
ok    liveness
ok    rpc-availability
ok    rpc-latency
ok    state-sync
ok    validator-downtime

7 SLOs, 7 with all four companions, 0 with gaps
```

Registration alone would be a dashboard of nothing, so node 0 of the
five-process devnet was scraped (`scripts/bft_devnet.py`, dev profile):
`maya_blocks_imported_total` 31, finality mean 0.55 s (p50/p99 bucket ≤ 1 s),
68 RPC calls ok and 3 errors (the script's deliberate past-tip probes),
`maya_sync_duration_seconds` 0.004.

## 1. slo-check (2026-09-26)

```
$ cargo xtask slo-check
GAP   finality-p50: metric `maya_finality_latency_seconds` is not registered by the node
GAP   finality-p99: metric `maya_finality_latency_seconds` is not registered by the node
ok    liveness
GAP   rpc-availability: metric `maya_rpc_requests_total` is not registered by the node
GAP   rpc-latency: metric `maya_rpc_duration_seconds` is not registered by the node
GAP   state-sync: metric `maya_sync_duration_seconds` is not registered by the node
GAP   validator-downtime: metric `maya_validator_missed_rounds` is not registered by the node

7 SLOs, 1 with all four companions, 6 with gaps
xtask: 6 SLOs lack a metric, dashboard panel, alert or runbook
```

Its first run failed all four companions on every row. `docs/SLO.md` named
a dashboard, alert file and runbooks that did not exist, and a
`maya_blocks_produced_total` the node never exported. This change adds:

- `infra/grafana/dashboards/slo.json`, one panel per SLO;
- `infra/grafana/alerts/slo.yaml`, one rule per SLO;
- the runbooks;
- the liveness row now names `maya_blocks_imported_total`, which the node
  does export.

The six remaining gaps are metrics that **cannot honestly exist yet**:

- finality and missed rounds need DAG-BFT and a validator set (ADR-015);
- sync duration needs a snapshot-sync path in the node;
- the two RPC metrics need a jsonrpsee metrics middleware, which is
  buildable and not built.

The check reads the node's metrics module, so it turns green only when the
node really emits them.

## 2. Runbooks

`docs/runbooks/README.md` lists 20. Rehearsed means a test or a measured run
exercised the failure and the recovery:

| Rehearsed (11) | Evidence |
|---|---|
| fell-behind, stuck-upgrade | `upgrade_rehearsal.rs`: 30 late nodes halt and catch up |
| chain-halt, coordinated-restart | `coordinated_restart_rehearsal.rs` (§3) |
| snapshot-corrupt | `state-sync` tests and the 100M sim with 4 lying peers |
| signer-unreachable | `signer_tests.rs` (pinning, restore-from-backup refused) |
| rpc-overload | `maya2c-rpc-load` curve (`reports/14-scale.md`) |
| handshake-flood, invalid-signature-spam, eclipse-suspected | `dos-guard` simulations (model, not the node) |
| invariant-guard-tripped | `exploit_replays.rs` |

**Not rehearsed (9):** disk-full, peer-starvation, clock-drift,
state-root-mismatch, db-corruption, memory-pressure, mempool-full,
deep-reorg (the reorg logic is tested, but not as an incident),
key-compromise.

## 3. Coordinated restart

```
$ cargo test -p custom-l1-node --profile ci --test coordinated_restart_rehearsal -- --nocapture
coordinated restart: 12 nodes; 9/12 operator signatures (quorum > 2/3) signed+verified in 15.4 ms; all nodes caught up and matched the record in 109 ms; 1 forked node rejected; resumed at 31 (total 0.4 s)
test twelve_nodes_agree_a_restart_point_verify_it_and_resume ... ok
```

- Twelve real nodes halt at heights 26–30.
- Operators sign `(height, state_root, block id)` with ML-DSA-65; three are
  unreachable, and nine is still over ⅔.
- Every node catches up to the record, checks its own root and id against
  it, and reopens its database.
- A thirteenth node that followed a fork fails the same check and is
  directed to resync.
- The next block is accepted everywhere.

The timings are one process on one machine. A real restart is bounded by
humans agreeing, not by 109 ms.

## 4. Not done

- `.deb`/`.rpm` packages and a hardware-checking config wizard.
- The Kubernetes operator (kube-rs), snapshot publishing service, public
  network health dashboard.
- Disaster-recovery rehearsals for region loss and ⅓-validator loss. There is
  no multi-region network, and no validator set.
- Local cluster with sentries, signers, RPC fleet, indexer and explorer. The
  local-docker target runs 12 unpeered nodes (`reports/10-launch.md`).
- The staging network and every "measure on staging" item. Plan only,
  awaiting "APPROVED: staging-network". An infracost estimate must come
  first, and there is none.

## Operator rehearsal on a real cluster (added 2026-09-27)

`infra/operator/` — a `MayaNetwork` CRD and a kopf operator — rehearsed by
`scripts/operator_rehearsal.sh` on k3d v5.9.0 in WSL2 (kubectl v1.37.1, kopf
1.44.6), a four-replica proof-of-work devnet with one miner:

```
[52s]  install: 4 replicas Ready
[64s]  install: every replica at height >= 25 (31 31 26 30)
[110s] restore: replica 3 rebuilt from replica 0's snapshot (height before 30, now 162); {"generation":1,"replica":3,"seconds":44.7}
[253s] upgrade: [{"replica":3,"seconds":35.4},{"replica":2,"seconds":35.4},{"replica":1,"seconds":35.4},{"replica":0,"seconds":35.5}]
[254s] agreement: all 4 replicas hold the same block at height 368
[254s] REHEARSAL PASSED in 254s
```

Full log: `reports/data/operator-rehearsal-2026-09-27.txt`. Install, snapshot
restore and rolling upgrade — the DONE WHEN's three — each pass, and the
upgrade is gated per replica on the chain advancing. Limits, stated: "v2" is
the same binary under a new tag (the rollout mechanics, not a protocol
change); the rehearsal chain is proof of work because a DAG-BFT node cannot
yet catch up past the DAG's GC window (ADR-027).
