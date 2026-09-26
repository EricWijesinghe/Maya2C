# Maya2C public benchmark report: draft

**Status: DRAFT, waiting for `APPROVED: publish benchmark report`.**

**Not independently reproduced.** The brief requires at least one party
outside the project to reproduce these figures before the report is promoted.
None has. Until then, every number below is the project's own measurement, on
one machine.

## Method

[docs/BENCHMARK_METHODOLOGY.md](../BENCHMARK_METHODOLOGY.md). Each figure
comes with its command, so a reproducer runs the same thing.

**Hardware.** Intel Xeon @ 2.80 GHz, 4 vCPU, 16 GiB, a virtio disk with
unknown IOPS, a Firecracker guest on a shared cloud host. Full record:
[reports/12-baseline.md](../../reports/12-baseline.md). Absolute figures
carry host noise.

**Build.** `release`, with thin LTO and 16 codegen units. This deviates from
the shipped `dist` profile (fat LTO), because the volume did not have the
space for a fat-LTO build.

## Results

| What | Figure | Command |
|---|---|---|
| Signature verification, hybrid ML-DSA-65 + SLH-DSA transfer, 1 core | 0.990 ms/tx (1,010 tx/s) | `cargo bench -p custom-l1-node --bench apply_pipeline` |
| Same, 4 cores | 0.324 ms/tx, 3.06× | same |
| Apply including verify, 1 core, RocksDB | 1.062 ms/tx (942 tx/s) | same |
| State work alone | 0.071 ms/tx (6.7% of apply) | same |
| Allocations per applied transfer | 9 | same |
| Write amplification | 19.3× | same |
| Peak RSS of the bench | 26.4 MiB | same |
| Transfer size on the wire | 13,215 B, 84.5% of it signatures | `reports/13-pq-weight.md` |
| Compression of blocks (zstd) | saves 13–16% | same |
| RPC reads per node | ~21k req/s; host plateau 28–30k | `maya2c-rpc-load`, `reports/14-scale.md` |
| Idle node memory | 80.3 MiB | `docs/NODE_TYPES.md` |
| Hybrid signature vs Ed25519 | ~17× slower to verify | `benches/competitive/run.sh`, `reports/21-strategy.md` |

## What this does not show

- **No end-to-end network TPS.** There is no public network, and the local
  12-node devnet's nodes are not peered (`reports/10-launch.md`). 942 tx/s is
  one core applying transfers, not a chain's throughput.
- **No comparison with other chains.** Every competitor row in
  `benches/competitive/run.sh` is SKIPPED, because their RPCs are unreachable
  from the measuring host. The only cross-system figure is the
  signature-verify ratio.
- **Cost of post-quantum signatures.** They are why the node is slower and
  heavier than a classical-signature chain on this hardware. The report says
  so rather than leaving it to a reader to discover.

## Before promotion

1. An outside party runs the commands on their own hardware and publishes the
   raw output.
2. The `dist`-profile figures replace the thin-LTO ones.
3. The owner writes `APPROVED: publish benchmark report`.
