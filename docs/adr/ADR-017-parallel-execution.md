# ADR-017: Parallel execution — optimistic with in-order validation first

**Status:** Accepted
**Date:** 2026-09-27

## Context

Master Prompt 12 §1 asks to choose between Block-STM (multi-version memory,
speculative execution, validation, re-execution), declared access lists (a
conflict graph from declared read/write sets), or a hybrid — with the hard
requirement that the parallel result is byte-identical to sequential
execution in block order, and gas independent of threads and re-executions.

## Decision

Build and measure two schedulers in `crates/parallel-exec`, both against the
same sequential reference:

1. **Optimistic + in-order value validation.** Execute all transactions in
   parallel against the pre-block state, recording read *values*; then in
   block order commit each one whose reads still equal the committed prefix,
   and re-execute inline any whose reads went stale. Correct by
   construction (a committed transaction always saw exactly the prefix
   state), needs no declarations, one pass.
2. **Declared-access waves.** A transaction joins the earliest wave after
   every earlier conflicting transaction; waves run in parallel. An
   undeclared access fails the transaction deterministically (base gas,
   no writes) in both the scheduler and its reference.

Full Block-STM — per-key version chains with ESTIMATE markers and a
collaborative scheduler that re-validates only dependents — is **not built**.
It wins over (1) exactly where (1) is weakest: high contention, where (1)
re-executes every conflicting transaction serially. It should be built only
when measured workloads show high-contention blocks are common *and* (2)
cannot be used because declarations are unavailable.

The mainnet choice is the **hybrid** the brief names: declared access lists
where transactions carry them (smart-account ops and transfers can: their
keys are known before execution), optimistic validation as the fallback for
contract calls whose touches are data-dependent.

## Consequences

- `tests/differential.rs`: parallel == sequential on **100,000** random
  blocks (2.46 M transactions, 993,076 optimistic re-executions) across 0–90%
  contention, 1–8 threads and deliberately lying declarations; gas identical
  at 1–32 threads.
- Speedup curves (1–32 threads × 0/10/50/90% contention) are in
  `reports/12-performance.md`, with this machine's 4 cores stated — threads
  past 4 measure the scheduler, not parallelism.
- Not in the node. The node's apply path is single-threaded; wiring either
  scheduler in means expressing `StateDB::apply_block`'s transaction effects
  as read/write sets over the state's keys.

## Revisit when

Loadgen runs against real contract traffic show a contention profile where
the optimistic re-execution share exceeds ~30% of transactions.
