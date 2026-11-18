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
   three crates that are off the consensus path as a whole (`archive`,
   `iso20022`, `hal/ebpf-net`). A float added to any other file of the node's
   `rpc` or `network` modules fails. `MUST_SCAN` names consensus paths that
   must keep contributing a scanned file.
4. **Fail closed.** Float names cover every primitive and SIMD lane type
   (`f16`, `f32`, `f64`, `f128`, `bf16`, `f32x4` ...), raw identifiers, the
   common float traits and wrappers (`Float`, `FloatCore`, `Real`,
   `OrderedFloat`, `NotNan`), and every float literal form. An ambiguous
   `#[cfg(test)]` field is scanned past its first comma rather than skipped,
   and a `use` that binds the name `test` (which would shadow the `#[test]`
   the scanner trusts) is itself a violation.
5. **ML runtimes through `Cargo.lock`.** The node's whole dependency closure is
   walked, so a transitive, renamed or target-specific ML crate fails.
6. **CI:** a step in the existing `reality ledger` job; it builds only xtask.

## Consequences

- Not caught: a float reached by type inference with no float token in a
  scanned file. `clippy::float_arithmetic` (type-aware) would close this and is
  a post-genesis item.
- WASM guest floats run in the consensus VM and are outside the invariant; the
  VM makes them deterministic (`crates/vm/src/config.rs`).
- An allowlist entry lists exact tokens, so it cannot hide a second float in
  the same file, and it needs a reviewer.

## Alternatives

- **`syn` visitor:** rejected, blind inside macros.
- **`clippy::float_arithmetic` only:** needs a full typed build of the node in
  CI and does not see a float type that is stored but not computed on. Worth
  adding beside this check, not instead of it.
- **Directory-level exclusions for `rpc`/`network`:** the first version did
  this; it left about 75 files unguarded, including block sync. Rejected.
