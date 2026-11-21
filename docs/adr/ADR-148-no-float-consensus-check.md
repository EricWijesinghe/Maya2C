# ADR-148: Invariant 36, checked by a token lexer over the node's link graph

**Status:** Accepted, 2026-11-14 (owner decision, Eric 2026-11-14, "I approve everything, proceed").
**Date:** 2026-11-18
**Related:** [docs/invariants.md](../invariants.md) invariant 36, [ADR-008](ADR-008-transparent-zk.md)
(retired invariant 20 with `crates/zkml`), [ADR-079](ADR-079-verifiable-compute.md),
[ADR-004](ADR-004-reality-ledger.md); PR #546.

## Context

"No float in a consensus rule" was Standing Order 4 and the prose of invariant
20, which retired with `crates/zkml`. Nothing checked it. Invariant 36 gives the
rule a number, and a number is only earned by a test that pins it.

## Decision

1. **A lexer, not `syn`.** `cargo xtask float-check` lexes source with
   `proc-macro2` (already in the lock) and inspects tokens, so macro bodies
   (`vec![0.5]`) are read. A `syn` visitor skips macro calls.
2. **Scope from the link graph.** The scanned crates are `bins/maya2c-node`
   and every path crate it reaches through normal, build and
   target-specific dependencies (`xtask/src/float_scope.rs`). A crate the node
   starts to link is scanned by default. A hand list missed six consensus
   paths in the first draft.
3. **Exclusions are files, not directories**, each with its own reason, except
   two crates that are off the consensus path as a whole (`archive`,
   `hal/ebpf-net`). A float added to any other file of the node's
   `rpc` or `network` modules fails. An entry, file or directory, under which
   no file holds a float fails as stale, so an exclusion cannot outlive its
   reason (`iso20022` was dropped this way: it held no float, and its amount
   parser builds payment `TxKind`s). That an excluded file is off the
   consensus path is a review judgement, not a tested fact. `MUST_SCAN` names
   consensus paths that must keep contributing a scanned file.
4. **Fail closed.** Float names cover every primitive and SIMD lane type
   (`f16`, `f32`, `f64`, `f128`, `bf16`, `f32x4` ...), raw identifiers, the
   common float traits and wrappers (`Float`, `FloatCore`, `Real`,
   `OrderedFloat`, `NotNan`), and every float literal form. An ambiguous
   `#[cfg(test)]` field is scanned past its first comma rather than skipped,
   and a `use` that binds the name `test`, or globs in a crate other than
   `std`, `core`, `alloc`, `crate`, `super` or `self` (either could shadow
   the `#[test]` the scanner trusts), is itself a violation. A file counts as
   a test only when every `mod` declaring it is under `#[cfg(test)]`.
5. **ML runtimes through `Cargo.lock`.** The node's whole dependency closure is
   walked, so a transitive, renamed or target-specific ML crate fails. Names
   are matched by family prefix (`burn-*`, `ort-*`, `tract-*`, `candle-*` and
   others), because a sub-crate such as `burn-ndarray` runs a model without
   depending on its facade.
6. **The dormant fee model is pinned by its callers.** fee-market's neural
   rule is reached only through `apply_block_fees*`, `resolve_base_fee`,
   `is_neural_active`, `next_base_fee_by_rule`, `FeeRule` and `MODEL_V1`; the
   check fails if any scanned file outside `crates/fee-market` names one. The
   node's base fee uses the linear `next_base_fee`, and the genesis
   `FeeConfig`'s `neural_activation_height` is validated but never stored, so
   it is not what keeps the model off.
7. **CI:** a step in the existing `reality ledger` job; it builds only xtask.

## Consequences

- Not caught: a float reached by type inference with no float token in a
  scanned file. `clippy::float_arithmetic` (type-aware) would close this and is
  a post-genesis item.
- WASM guest floats run in the consensus VM and are outside the invariant; the
  VM makes them deterministic (`crates/vm/src/config.rs`).
- An allowlist entry lists exact tokens, so it cannot hide a second float in
  the same file, and it needs a reviewer.
- The model half is a denylist plus a caller check: an integer model elsewhere,
  or a model run off-node whose stored output a rule later reads, is not
  detected.

## Alternatives

- **`syn` visitor:** rejected, blind inside macros.
- **`clippy::float_arithmetic` only:** needs a full typed build of the node in
  CI and does not see a float type that is stored but not computed on. Worth
  adding beside this check, not instead of it.
- **Directory-level exclusions for `rpc`/`network`:** the first version did
  this; it left about 75 files unguarded, including block sync. Rejected.
