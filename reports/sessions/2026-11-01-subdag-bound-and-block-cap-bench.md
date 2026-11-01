# 2026-11-01: sub-DAG decode bound (CON-13) and block-cap apply measurement

Branch `fix/subdag-decode-bound`, based on `origin/fix/max-block-bytes` (c219bd5c, PR #276).

## Status

- Task A (decode bound): code and tests written. **NOT VERIFIED**: the build did not finish.
- Task B (apply time per MiB): **NOT MEASURED**. It needs a release build of the bench, which
  this session could not reach. No figure is reported here.

## Task A: what was verified by reading

- Decode was unbounded in two places, each decoding every batch payload of the committed sub-DAG:
  - `crates/node/src/consensus/bft/builder.rs` (old `ordered_transactions`, lines 50-59).
  - `crates/node/src/consensus/bft/driver.rs` (old own-payload loop, ~line 737): decoded every own
    payload again to release queued copies.
- The protocol does not bound a sub-DAG:
  - A commit's sub-DAG is the causal history not yet ordered (`crates/dag-bft/src/commit.rs`,
    `order_history`, `commit_chain`).
  - The DAG is pruned only to `last_committed_round - GC_DEPTH` (`crates/dag-bft/src/validator.rs`
    `progress`, `GC_DEPTH = 50` at line 24).
  - The gap between commits follows leader liveness, which the engine does not bound.
  - So the rounds in one sub-DAG are `gap + 50`, and the worst case is that count × 100 vertices
    × 4 MiB (`MAX_BATCH_BYTES`, `store.rs:53`; `MAX_COMMITTEE`, `store.rs:66`). No ceiling exists.

## Task A: change

- `core::block::decode_cut` (`crates/node/src/core/block.rs`): the longest prefix whose raw bytes
  fit the budget. It stops at the first payload past the budget (first-fit would pass it over).
- `builder::decode_within`: dedupes raw payloads first (repeats cost no budget, no decode), applies
  `decode_cut` with `DECODE_BUDGET = MAX_BLOCK_BYTES` (5,242,880), then decodes only that prefix.
- Driver: `queued` maps txid to blake3 of the raw payload. Own payloads in the sub-DAG are matched
  by hash, so a payload past the cut is released without a decode and stays in the pool.
  `Step.dropped` is now ids (its only consumer, `bins/maya2c-node/src/bft.rs`, used only ids).
- Spec: CON-13 in `spec/05-consensus.md`; oracle `spec-ref::consensus::decode_cut`; five vectors
  in `spec/tests/consensus.json` (appended, diff is additions only); conformance arm
  `decode_cut` in `crates/node/tests/consensus_conformance.rs`.
- ADR-072: section "Sub-DAG decode bound (CON-13)" added.

Worst-case decode per block after the change: 5,242,880 payload bytes. Hashing of the sub-DAG's own
payloads in the driver is linear in the sub-DAG and decodes nothing.

## Task A: tests (written, not run)

- `a_walk_across_many_stalled_rounds_decodes_within_the_budget` (builder.rs): 100 × 1 MiB distinct
  payloads; expects 5 MiB charged, and two walks of one order agree. Fails on the old code, which has
  no budget (the old code has no way to report bytes decoded).
- `a_repeated_payload_is_decoded_and_charged_once`.
- `transactions_past_the_budget_are_left_for_a_later_walk`: the leftover is decoded by the next walk,
  once.
- Block identical across two nodes: covered only as "two walks of one order agree". There is no
  two-node test; the driver path has no new test.
- Unsigned payloads are used in the builder tests. Whether `from_bytes` accepts them is unchecked.

## Verification: what happened

- `CARGO_BUILD_JOBS=1 cargo test -p custom-l1-node --lib --no-run` was started in the worktree
  (cold target dir). Log: `D:/Temp/claude-dag-warm.log`. After about 7 hours it had compiled 69 of
  roughly 1,167 lockfile packages (`Cargo.lock`). It was still running when this report was written.
- Machine state during the build: `MsMpEng` (Defender) at 71,630 s CPU, 8.0 GiB of 32.9 GiB free,
  Memory Compression at 4.9 GB working set (`Get-Process`, 2026-11-01).
- `rustfmt --edition 2024 --check` is clean on the changed Rust files. This is a parse and format
  check only.
- `spec/tests/consensus.json` was extended by a script (`python -I`) that first confirmed a
  round trip reproduces the committed bytes. The generator (`spec-vectors`) was not run.
  `cargo run -p maya-spec-ref --bin spec-vectors -- --check` is the confirmation step, still open.
- NOT VERIFIED: compile, `cargo test`, the conformance test, `spec-vectors --check`, clippy.

## Task B: not measured

No apply-per-MiB figure exists from this session. The ADR-072 figures remain derived, not measured.
Recommendation: keep `MAX_BLOCK_BYTES` at 5 MiB, provisional. Nothing measured contradicts it, and
the change is not justified without a measurement. The measurement is the gate for moving ADR-072 to
Accepted. Record: `reports/bench/2026-11-01-apply-per-mib.md` (blocked, command and fields listed).

## Residual

- A sub-DAG whose first 5 MiB are non-executing payloads leaves the executable ones after them to a
  later block. Not a safety issue; lower throughput under that attack.
- The gap in a commit is not bounded by the protocol. The cut bounds decode work, not the number of
  rounds a commit spans.
