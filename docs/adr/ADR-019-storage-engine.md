# ADR-019: Stay on RocksDB; no custom state store until it is the measured bottleneck

**Status:** Accepted
**Date:** 2026-09-27

## Context

Master Prompt 12 §3 asks for RocksDB to be measured as configured, tuned,
the flat key-value state separated from the authenticated structure, and a
benchmark-driven decision between tuned RocksDB and a custom append-only /
io_uring store (Monad-style) — "build a prototype only if RocksDB is proven
to be the top bottleneck after tuning".

## Evidence available

- The node commits each block in one RocksDB `WriteBatch` with its undo
  journal and the tip pointer (`StateDB::apply_journaled_with`), so a crash
  at any instruction leaves either the old or the new state:
  `crates/node/tests/crash_consistency_tests.rs` SIGKILLs a committing node
  1,000 times and every restart is consistent (`reports/12-performance.md`).
- Block/cache sizes are operator-tunable (`[storage]` in `config.toml`,
  `StateDB::open_tuned`): 512 MiB block cache and 64 MiB write buffer by
  default.
- **No profile shows RocksDB as a top-5 bottleneck**, because no end-to-end
  load test of the node at scale has been run (the node's throughput is
  bounded first by ~0.1 s hybrid signing per transaction on the client side
  and PoW block production). Write amplification and p99 read latency under
  load were not measured.

## Decision

1. Stay on RocksDB. A custom store is a large, risky component whose
   justification must be a measurement this tree does not have.
2. WAL is **not** fsynced per write (RocksDB default). Process crashes lose
   nothing (the crash test); a power loss may lose the last unsynced batches,
   which a node recovers from by re-syncing the lost blocks from peers — they
   were already finalized by the network. Validators that sign votes must
   fsync their *slashing-protection* records before releasing a signature
   (Master Prompt 16), which is a different file with a different rule.
3. Separate flat state from the authenticated tree when DAG-BFT execution
   lands: the parallel executor (ADR-017) reads flat keys; roots are computed
   per block over the touched keys.

## Revisit when

A loadgen run against the node shows compaction stalls or read latency in
the top five costs per block.
