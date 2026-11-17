# ADR-146: Parallel apply is a verify phase first; stateful speculation waits for a measured need

Status: Proposed (post-genesis, v3 P1; dark at `PARALLEL_APPLY_ACTIVATION_HEIGHT = u64::MAX`)
Date: 2026-11-17

## Context

v3 P1 (`.claude/plans/v3-major-update.plan.md`) asks to wire `parallel-exec`
into block application behind an activation height, with the sequential path
as the reference, and to drop the item if no multi-core gain is measured.

`maya-parallel-exec` (ADR-017) is a model, not an executor the node can call.
It runs `maya_loadgen::Tx` over a `BTreeMap<u64, u64>` and learns each
transaction's read set by running a pure function of the values read. The
node's apply path is `StateDB::stage_transaction` over an `Overlay`: it reads
through RocksDB, writes vault records and fee tallies, and runs `apply_kind`
for typed payloads (channels, DEX, governance, contracts) before the
end-of-block `settle_*` passes. None of that records its reads. Linking the
crate as it stands would compile the loadgen model into the node and execute
nothing real.

What the crate's optimistic scheduler parallelises is the part of a
transaction that does not depend on the committed prefix. In a node
transaction that part is signature verification: hybrid verification was 93 %
of apply in `reports/12-performance.md`, and it reads no state.

## Decision

- `stage_block` runs one parallel phase when
  `BlockContext::parallel_apply_active()`: `prewarm_verification`, the same
  scoped-thread fill of the verified-signature cache the builder already uses
  in `select_applicable_within`. The sequential loop that follows is unchanged
  and still verifies every transaction, in order, through the cache.
- The result cannot differ from the serial path: the cache stores successes
  only, keyed on the chain tag and the full wire encoding, and v7 admissibility
  and v8 multisig run on every call (`state::verified`). An unbound chain's
  error is left to the sequential loop so both paths return the same error.
- `PARALLEL_APPLY_ACTIVATION_HEIGHT = u64::MAX`. Because the phase is not a
  state-root format change, the height is a switch, not a fork; it stays dark
  until the measurement below says it earns a height.
- Stateful speculation (executing `stage_transaction` against a snapshot and
  validating read values in block order) is **not** built. It needs read
  tracing through `load`, `apply_kind` and every record write, and a
  divergence there is a state split. It is reconsidered only when a measured
  profile shows state work, not verification, bounding apply.

## Evidence

- `crates/node/tests/parallel_apply_tests.rs`: two databases, phase off and
  on, the same seeded blocks (conflicting and non-conflicting) plus a forged,
  a replayed and an overspending variant of each; equal roots and equal errors.
- `crates/node/benches/parallel_apply.rs`: apply time off vs on, cold and warm
  cache. Figures and the machine they came from:
  `reports/bench/2026-11-17-parallel-apply.md`.

## Consequences

- No wire, state or consensus-rule change, so no spec rule or conformance
  vector; invariant 24 (the root check) is what any divergence would hit.
- The 100,000-block figure of ADR-017 is the model's. The node-level
  differential test runs far fewer blocks, because each transaction carries a
  real hybrid signature (about 135 ms to sign, `reports/02-crypto.md`).
- Replayed testnet history was not run through both paths: NOT VERIFIED.
