# 10 — Launch pipeline

**Date:** 2026-09-26
**Branch:** `claude/task-0g86kl`
**Brief:** Master Prompt 10 — *"`./deploy-production.sh --dry-run` completes
end-to-end against a local kind/k3d cluster with 12 simulated nodes;
reports/10-launch.md and LAUNCH.md exist."*

## 0. Update 2026-09-27: k3d, to the letter — **met**

On a machine with ghcr.io access (Windows 11 host, WSL2 Ubuntu 24.04, Docker
29.1.3, k3d v5.9.0, kubectl v1.37.1; commit `e57e1ec`), the brief's path ran
unchanged:

```
$ NODES=12 ./deploy-production.sh --target local-k3d
   k3d cluster maya2c-local (1 server, 2 agents)             ok
   namespace + network policy                                ok
   build maya2c-node (--profile ci)                          ok
   import image into k3d                                     ok
   apply overlay local-k3d (12 replicas)                     ok
   wait for 12 nodes Ready (timeout 15 min)                  ok (12/12)
   smoke: get_supply over JSON-RPC on every node             ok (12/12 answered)
   |   mode .......... dry-run      target ........ local-k3d      |
   |   nodes ......... 12           elapsed ....... 636s           |
```

Full log: `reports/data/deploy-local-k3d-2026-09-27.txt`. The DONE WHEN is met
to the letter. What it proves is unchanged from §1: the pipeline, not a
network — the twelve pods run the proof-of-work devnet genesis of the overlay
and are not peered into one DAG-BFT committee.

## 1. Result (2026-09-26, cloud sandbox)

**The k3d path could not run in this environment. A Docker-only path with
the same binary, image, genesis and smoke test completed with 12 of 12 nodes
answering.** The brief names k3d/kind specifically, so the done-when
condition is **met in substance, not to the letter**, and the reason is
recorded below rather than papered over.

```
$ NODES=12 SKIP_BUILD=1 ./deploy-production.sh --target local-docker --keep

== Phase A: pre-flight [........................]
   mode=dry-run target=local-docker nodes=12                 ok
   tool: cargo                                               ok
   tool: git                                                 ok
   tool: docker                                              ok
   docker daemon                                             ok
   features.toml claims backed (cargo xtask coverage)        ok
   formal proofs (Lean models, if lean is installed)         ok
   secrets present (.env.production.age)                     SKIP (not needed for dry-run/local-docker)

== Phase B: infrastructure [######..................]
   docker network maya2c-local                               ok

== Phase C: hardening [############............]
   containers: read-only rootfs, cap-drop ALL, no-new-privs  ok (applied per container in phase D)

== Phase D: rollout [##################......]
   build maya2c-node (--profile ci)                          ok (reused target/ci/maya2c-node)
   image maya2c/node:local                                   ok
   start 12 nodes (maya-seed-0 .. maya-seed-11)              ok
   smoke: get_supply over JSON-RPC on every node (timeout 5 min)ok (12/12 answered)

   +--------------------------------------------------------------+
   |                 MAYA2C LAUNCH CERTIFICATE                    |
   |   mode .......... dry-run                                    |
   |   target ........ local-docker                               |
   |   nodes ......... 12                                         |
   |   elapsed ....... 24s                                        |
   |   A dry run proves the pipeline, not a network. No value     |
   |   moved and no public endpoint exists.                       |
   +--------------------------------------------------------------+
real	0m24.306s
```

`SKIP_BUILD=1` reused a `ci`-profile binary built minutes earlier by the
same script (its first attempt failed on a Docker Hub 429, after the build
step had passed).

## 2. What the 12 nodes are, and are not

Each container runs the real `maya2c-node` binary on the genesis from
`infra/k8s/base/configmap.yaml`, read-only root filesystem, all capabilities
dropped, `no-new-privileges`. Each answered:

```
{"jsonrpc":"2.0","id":1,"result":{"total":0,"circulating":0,"shielded":0,"burned":0}}
```

**They do not form a network.** The genesis has no bootnodes and the node
logs `mining: disabled`; every node is a lone chain at height 0. The k8s
overlay has the same property — the smoke test in both paths checks that
the process boots, binds and serves RPC, not that it peers or reaches
consensus. A multi-node consensus test exists elsewhere
(`crates/dag-bft/tests/modes_sim.rs`, simulated), but nothing here wires
those rules into the binary: `bins/maya2c-node` refuses to start a
`production` build because DAG-BFT is not connected (ADR-015).

## 3. Why k3d did not run — measured

| Attempt | Outcome |
|---|---|
| `k3d cluster create` (script, 3 retries) | hung; server container stuck in `Created`, `exec /bin/k3d-entrypoint.sh failed` when started by hand |
| `k3d cluster create --verbose` | proxy log: **597 refused CONNECTs to `pkg-containers.githubusercontent.com`** — k3d's `k3d-tools` / `k3d-proxy` images come from ghcr.io, which this environment's egress policy blocks |
| `rancher/k3s` run directly in Docker | server Ready in 8 s |
| …first pod | `rancher/mirrored-pause` pull failed: containerd inside k3s does not trust the egress proxy's CA; then `Evicted` on the default 10 % ephemeral-storage threshold |
| …images preloaded with `ctr import`, eviction threshold relaxed | `runc create failed: can't get final child's PID from pipe: EOF` — this sandbox's kernel refuses a nested container runtime |
| …same with `--cgroupns=host` and `/sys/fs/cgroup` bind | same `runc` failure |

The first failure is policy, the last is the kernel; neither is a defect in
the manifests. `--target local-k3d` is unchanged and remains the path on
any workstation with Docker and ghcr access.

## 4. What changed in `deploy-production.sh`

- `--target local-docker`: a Docker network and `$NODES` containers, cleaned
  up by label on exit, or kept with `--keep`; the rollback trap removes them
  on failure. The genesis is extracted from the same ConfigMap the k8s path
  mounts, so both local targets boot the same chain.
- The phase bar was rewritten as a plain loop. The edit landed while a k3d
  run was in flight, so that run was discarded rather than reported.
- The pre-flight's Lean step now runs (Lean installed at `/opt/lean`).

## 5. The cloud path

Unchanged, and not exercised: a dry run against `--target cloud` validates
terraform and runs `ansible --check` only where those tools exist (neither
is installed here, so both are `SKIP`). A real deploy needs `--apply` **and**
`APPROVED=<phase,…>` per phase; nothing in this session spent money or
touched a server (Standing Order 6).

Secrets: `scripts/configure_environment.sh` reads them with `read -s`,
passes them to `curl` via `-K -` (never argv), and stores them
age-encrypted; `.env.production*`, `terraform.tfvars` and `production.ini`
are git-ignored.

## 6. Open

1. Run `--target local-k3d` on a machine with ghcr access and record it.
2. Give the local genesis bootnodes so the smoke test can check peering.
3. Helm chart: not written (`helm` unavailable here); kustomize only.
