# Benchmark methodology

How every performance figure in `reports/` is produced, so it can be
checked. Master Prompt 19 §6.

1. **Hardware record with every number.** CPU model and count, memory,
   disk, kernel, toolchain (`reports/12-baseline.md` is the template).
2. **Profile stated.** `ci` (dependencies optimized, node crate not) or
   optimized (`release`, and any environment overrides such as thin LTO
   written out). A figure without its profile is not comparable.
3. **The exact command, and its raw output.** Numbers are copied from the
   output, never retyped from memory (Standing Order 1).
4. **What the benchmark cannot see is printed by the benchmark.** Examples:
   `apply_pipeline` prints "NOT MEASURED" where `/proc` is missing;
   `certificate` prints "NOT BUILT" for options (b) and (c); the DA dispersal
   comparison prints "MODEL".
5. **Targets are not tuning goals** (Standing Order 2). A result below a
   brief's number is reported as below it.
6. **Simulations say so** (`[SIM]` in their output) and state their model:
   tick length, cost per operation, what stands in for what.
7. **Comparisons on one machine only.** The regression gate
   (`scripts/bench_gate.py`) compares a run only with the same runner's
   history, and it is not live until a fixed-spec runner exists.

## Reproducing

| Figure | Command |
|---|---|
| Node verify/apply, allocations, write amplification | `cargo bench -p custom-l1-node --bench apply_pipeline` |
| Parallel execution speedup | `cargo bench -p maya-parallel-exec --bench speedup` |
| Signature and certificate costs | `cargo bench -p maya-crypto-pq --bench certificate` |
| DA encoding | `cargo bench -p maya-da --bench da_bench` |
| Handshake reject/accept | `cargo bench -p maya-dos-guard --bench handshake_gate` |
| RPC throughput | `maya2c-rpc-load --targets … --threads 16 --secs 10` |
| 100M-account state sync | `cargo run --release -p maya-state-sync --bin state-sync-sim` |
