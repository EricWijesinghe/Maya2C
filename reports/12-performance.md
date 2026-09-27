# 12 — Execution performance

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 12
Hardware record and profiles: [12-baseline.md](12-baseline.md).

## 1. The top five bottlenecks, with evidence

Ranked by share of measured time on the path a block takes through a node.

| # | Bottleneck | Evidence | Status |
|---|---|---|---|
| 1 | **`apply_block` verifies every signature again.** Mempool admission already verified it. | verify 0.990 of 1.062 ms/tx = **93 %** of apply, optimized (96 % in `ci`) | **fixed** 2026-09-27: `state::verified` verify-once cache, plus parallel pre-verification in the builder (`reports/13-pq-weight.md` §9) |
| 2 | **Hybrid signature verification itself.** | 1,010 verifications/s on one core; 3,086/s on 4 (x3.06) | inherent to the post-quantum choice; Master Prompt 13 |
| 3 | **Skewed account access defeats parallel execution.** | Zipf(1.0) accounts: best speedup 1.24x at any hot-key share; uniform accounts, heavy work: **3.56x** on 4 cores (§2) | measured; a workload property, not a scheduler bug |
| 4 | **Scheduler overhead at transfer granularity.** | at ~4.4 µs/tx of work, optimistic on 1 thread runs at 0.87x of sequential; `waves` on 2 threads at 0.30x (it spawns per wave) | measured; sequential below a work threshold is the fix |
| 5 | **Fixed per-commit write cost on small blocks.** | 118,784 B written for 6,144 B of account records: **19.3x** over 4 blocks | measured at tiny scale only; steady state not measured |

### Designing the fix for #1

The brief's rule: "signature verification happens once, at ingest … cached
by tx hash so execution never verifies twice". For a v5 transaction the
node's `txid` is `blake3(signing_bytes ‖ ml_dsa_sig ‖ slh_dsa_sig)`. That
covers the exact bytes that verified, so a cache keyed by txid is sound. An
earlier draft of this report said otherwise, from a misreading of
`Transaction::txid`. The spec conformance suite caught the misreading
(`reports/15-spec.md`).

It is not built here, for two reasons:

- It changes `stage_transaction`, the consensus chokepoint where TX-1 is
  enforced. CLAUDE.md requires a `rust-reviewer` and `security-reviewer` pass
  for that.
- The cache's key and its bound need to be decided as a pair. v8 multisig
  ids deliberately exclude approvals, so the key there must differ.

Expected effect, from the numbers above: block application about 1.06 →
0.07 ms/tx for pre-verified transactions. That is a projection, not a
measurement.

## 2. Parallel execution — speedup curves

`cargo bench -p maya-parallel-exec --bench speedup`, optimized (bench
profile), 10,000 transactions per block, 4 cores. "w2000" / "w20000" is
synthetic work per transaction: an integer mix standing in for VM cost, about
4.4 µs and 42 µs. `redo` is optimistic re-executions; `Nw` is the waves
scheduler's wave count.

Selected rows (the full 150-row table is reproducible with the command above;
the 1–32-thread sweep is in the raw output):

```
             curve threads    scheduler        ms        tx/s   speedup    redo
 zipf1 hot0% w2000       1   sequential      46.4      215291      1.00       -
 zipf1 hot0% w2000       4   optimistic      37.5      266832      1.24    4981
 zipf1 hot0% w2000       4        waves     106.8       93672      0.44   1325w
zipf1 hot10% w2000       4   optimistic      37.6      265758      1.19    5238
zipf1 hot50% w2000       4   optimistic      44.0      227342      1.05    6258
zipf1 hot90% w2000       4   optimistic      46.7      214096      0.99    7251
     uniform w2000       1   optimistic      54.8      182550      0.87     745
     uniform w2000       4   optimistic      22.1      453077      2.16     745
     uniform w2000       4        waves      26.3      379768      1.81      4w
     uniform w2000      32   optimistic      27.3      366170      1.74     745
    uniform w20000       4   optimistic     143.7       69581      2.91     745
    uniform w20000       4        waves     117.7       84980      3.56      4w
    uniform w20000      32        waves     132.8       75302      3.15      4w
zipf1 hot50% w20000       4   optimistic     373.5       26774      1.11    6258
zipf1 hot50% w20000       4        waves     380.7       26268      1.09   1450w
```

What it says:

- The brief's 0 / 10 / 50 / 90 % contention levels are hot-key shares. Over
  the default Zipf(1.0) account distribution, the top account alone takes
  about 8 % of draws, so "0 %" still conflicts on 1 in 2 transactions (4,981
  re-executions in 10,000). The uniform curves are the control that
  separates the two effects.
- 8, 16 and 32 threads on 4 cores measure the scheduler, not parallelism; the
  curve flattens or falls past 4, as it should.
- Neither scheduler beats sequential on Zipf(1.0) accounts by more than 1.24x
  at any work size. This is the most important number for choosing between
  them, and it argues for access-list hints (ADR-017's hybrid) on hot
  accounts.
- **ADR-017's revisit trigger has fired on synthetic traffic.** The ADR says
  to revisit when the optimistic re-execution share exceeds ~30 %. On the
  Zipf(1.0) curves it is 50–73 % (4,981–7,251 of 10,000). The trigger names
  *real contract traffic*, which does not exist yet, so the ADR stands. This
  is reported here so that the first real traffic measurement is compared
  against it. At heavy work on uniform accounts, `waves` also beats
  optimistic (3.56x vs 2.91x), the reverse of the ADR's ordering.

**Correctness, the hard requirement:**

```
$ cargo test -p maya-parallel-exec --profile ci --test differential -- --nocapture
test gas_does_not_depend_on_thread_count ... ok
100000 blocks, 2457783 txs, 993076 optimistic re-executions: parallel == sequential every time
test parallel_equals_sequential_on_100k_random_blocks ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 61.96s
```

## 3. Crash consistency

```
$ cargo test -p custom-l1-node --profile ci --test crash_consistency_tests -- --nocapture
test kill_nine_during_commit_always_restarts_consistent ... 1000 kill -9 runs: every restart consistent; final height 16053; 446 runs committed at least one block
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 115.15s
```

Covers process death at any instruction, including mid-`WriteBatch`. It
does not cover power loss: the WAL is not fsynced per write (ADR-019).

## 4. Hot-path hygiene

- **Allocations per transaction** (counting allocator in `apply_pipeline`):
  1 in verification, 8 in state work, 9 total in `apply_block`. Execution
  directive 3 treats this as a target, not a description: the journal and
  RocksDB's owned-buffer interface allocate. No change was made.
- **PGO: measured, no gain, not adopted** (§7). Zero-copy decode, an
  allocator comparison and BOLT: not done.

## 5. Regression gate

`scripts/bench_gate.py` parses the two tracked benchmarks into 8 metrics and
compares each to the last value for the same runner in
`benchmarks/history.csv`. It fails on a regression over 5 %, unless the metric
is listed in `benchmarks/OVERRIDES.md` with a reason. Self-test:

```
  apply.apply_1t_tx_s                                700.00   was 822.00   -14.8%   REGRESSED
bench_gate: FAIL — a tracked metric regressed more than 5%          (exit 1)
  apply.apply_1t_tx_s                                700.00   was 822.00   -14.8%   REGRESSED (overridden)
bench_gate: pass                                                      (exit 0)
```

The `bench-gate` job in `nightly.yml` runs only on the self-hosted runner
named by the repository variable `BENCH_RUNNER`. **No such runner exists, so
the gate is not live.** A shared GitHub runner's noise exceeds 5 %. The
runner spec to record when one is registered is the table in
[12-baseline.md](12-baseline.md). No history row is committed: this sandbox
is not a fixed-spec machine, and a baseline from it would make the first real
run look like a regression or an improvement it is not.

## 6. Pipelining, async execution, storage

- Pipelined stages with bounded channels: **not built** as such. The
  signature check moved: verify-once and parallel pre-verification (row 1).
  Block building no longer stages a block twice (§7).
- Asynchronous execution: ADR-018. Synchronous for v1, D = 2 later.
- Storage engine: ADR-019. Stay on RocksDB. Separating the flat state from the
  authenticated tree was not done.

## 7. PGO and preview reuse, measured (2026-09-28, Windows 11 workstation)

Workload: `crates/node/examples/bft_tps.rs 2000 5`, the `perf` profile, four
DAG-BFT validators in one process, 10,000 transfers verified, executed and
committed on every node. Three runs each; run-to-run spread on this machine
is about ±15 %, so only medians are compared.

**PGO** (`cargo xtask pgo`: plain, instrumented + training run,
`-Cprofile-use`, each in its own target directory):

```
pgo: bft_tps 2000 accounts x 5 (perf profile), 3 runs each
  baseline  [1028.0, 1053.0, 1438.0] tx/s  median 1053
  pgo       [1532.0, 1031.0, 1051.0] tx/s  median 1051
  ratio     0.998
```

There is no measurable gain, so PGO is not adopted. The cost is not in branch
layout; it is in the work below.

**Preview reuse** (`state::preview`): a node building a block staged it for
the header's state root, then staged it again when inserting it. The preview's
overlay and root are now kept, and reused only for the identical block,
context and committed state (a write generation). The header's root is still
compared on every apply. The first version was never hit: `insert_block`
stores the block body, a block-store write that bumped the generation.
Block-store writes cannot change what staging computes, since no state module
reads the block store, so they no longer count.

```
= 1084 tx/s   building + inserting, 4 nodes: 6.13 s   reused the builder's preview: 44, staged afresh: 0
= 1113 tx/s   building + inserting, 4 nodes: 6.04 s   reused the builder's preview: 44, staged afresh: 0
= 1099 tx/s   building + inserting, 4 nodes: 6.09 s   reused the builder's preview: 44, staged afresh: 0
```

Every apply reused its preview. Block building and insertion went from
6.8 s (`reports/04-consensus.md` §6) to 6.1 s, and the median from 1,053 to
1,099 tx/s (+4 %).

**Where the rest of the time goes, measured.** The builder was timed phase by
phase (temporary instrumentation, removed), summed over the four nodes, in
one 1,084 tx/s run:

| phase | seconds | what it is |
|---|---|---|
| select (`select_applicable`) | 2.24 | mostly `prewarm_verification`: first-time ML-DSA verification of every transaction on every core |
| insert (`insert_block`, preview reused) | 1.51 | undo capture, balance-change index, the RocksDB batch |
| preview (`candidate_block_sealed`) | 1.31 | staging the kept transactions again (signatures now cached), settle passes, root |
| decode (`ordered_transactions`) | 0.70 | decoding every certified batch's transaction bytes |

**The state root is not the bottleneck at this scale**, which corrects the
guess in `reports/04-consensus.md` §6. `examples/state_root_cost.rs`:

```
   2000 accounts: scan     0.49 ms, full root     0.88 ms (56% of it the scan), 0.44 µs/account
  20000 accounts: scan     3.74 ms, full root     9.98 ms (37% of it the scan), 0.50 µs/account
 200000 accounts: scan    32.74 ms, full root    96.13 ms (34% of it the scan), 0.48 µs/account
```

A root costs about 0.48 µs per account, linearly: under a millisecond here,
96 ms at 200,000 accounts, about 5 s at 10 million. So an incremental state
tree is a requirement **before** a large state, not a fix for this benchmark.
Here, first-time post-quantum signature verification is the largest single
cost. It is inherent to the scheme. It spreads across processes
(`reports/26`, split validator 36,530 tx/s) and shrinks with aggregation
(`reports/13`). In this benchmark every one of four in-process nodes pays it
for every transaction.

