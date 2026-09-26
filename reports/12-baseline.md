# 12 — Execution baseline

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 12 §0

## Hardware record

| | |
|---|---|
| CPU | Intel Xeon @ 2.80 GHz, 4 vCPU, 1 thread per core, 33 MiB L3 |
| Memory | 16 GiB |
| Disk | virtio block device (`/dev/vda`), 252 GB volume; type and IOPS not exposed |
| Kernel | Linux 6.18.44 (Firecracker guest) |
| Toolchain | rustc 1.99.0-nightly (da80ed070 2026-07-14) |
| Other load | none of my own; a shared cloud host, so absolute figures carry host noise |

Two build profiles are reported, and every table says which:

- **`ci`**: dev semantics. Every dependency and the crypto crates are at
  opt-level 3 via `[profile.dev.package.*]`; the node crate is at 0 with
  debug assertions.
- **optimized**: `release` with `CARGO_PROFILE_RELEASE_LTO=thin
  CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16` set in the environment for this run.
  Fat LTO was not used, to keep the build within this volume's free space.
  That is a stated deviation from the shipped `dist` profile.

## Node transaction path — `crates/node/benches/apply_pipeline.rs`

400 real hybrid (ML-DSA-65 + SLH-DSA-SHA2-128s) transfers, 16 senders, 100
per block, applied through `StateDB::apply_block` on RocksDB.

```
$ CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 cargo bench -p custom-l1-node --bench apply_pipeline
apply_pipeline: 400 hybrid transfers, 100/block, 4 cores, optimized
verify   1 thread :    0.990 ms/tx      1010 tx/s  1 allocs/tx
verify  4 threads:    0.324 ms/tx      3086 tx/s  (x3.06)
apply    1 thread :    1.062 ms/tx       942 tx/s  9 allocs/tx (includes verify)
  state work alone:    0.071 ms/tx  (6.7% of apply; 8 allocs/tx)
write amplification: 118784 B to the block layer / 6144 B of account records rewritten = 19.3x (5286000 B of transactions applied)
peak RSS: 26.4 MiB
```

```
$ cargo bench -p custom-l1-node --profile ci --bench apply_pipeline
apply_pipeline: 400 hybrid transfers, 100/block, 4 cores, debug-assertions ON (not an optimized build)
verify   1 thread :    1.171 ms/tx       854 tx/s  1 allocs/tx
verify  4 threads:    0.317 ms/tx      3158 tx/s  (x3.70)
apply    1 thread :    1.216 ms/tx       822 tx/s  9 allocs/tx (includes verify)
  state work alone:    0.045 ms/tx  (3.7% of apply; 8 allocs/tx)
write amplification: 143360 B to the block layer / 5286000 B of transactions applied = 0.03x
peak RSS: 29.6 MiB
```

The `ci` run's write-amplification line divided by transaction bytes. `StateDB`
stores accounts, not transaction bodies, so the denominator was changed to the
account records a block rewrites before the optimized run. The byte count
itself is comparable.

**How to read "state work alone":** it is `apply − verify`, taken from two
separate timed passes, so it carries the noise of both. The 0.045 vs 0.071
ms/tx difference between profiles is within that noise, not a regression.

**Throughput, as the Production Standing Orders define it** (valid
transactions applied and committed per second, one node): **942 tx/s** on
one core, optimized. This is block validation, not network throughput. No
consensus, gossip or multi-node figure is included here.

## Not measured (and why)

| Brief item | Status |
|---|---|
| 4-validator local cluster TPS, finality p50/p99 | not run: the node does not run DAG-BFT (ADR-015), and the local deploy's nodes do not peer (`reports/10-launch.md`) |
| perf / flamegraph / samply profiles | no profiler installed on this machine; the stage timings above stand in as the evidence |
| CPU per stage beyond verify vs apply | not instrumented |
| RocksDB compaction stalls, p99 read latency under load | not measured |
