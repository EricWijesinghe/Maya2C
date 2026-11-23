# 11. Contract instantiation gas — v0.1.0

What a contract call is charged before `invoke` runs, for setting up the
module's linear memory. Reference: `crates/vm/src/config.rs`
(`deterministic_config`). Vectors: `spec/tests/vm_instantiation.json`,
replayed on every execution tier by
`crates/vm/tests/tier_differential_tests.rs`
(`the_vm_instantiation_vectors_hold_in_every_tier`).

Gas is wasmtime fuel, the same unit as the rest of contract execution. The
section exists because one piece of it once depended on the validator's
operating system rather than on the module: wasmtime's copy-on-write memory
image let Linux skip the metered copy of data segments while Windows copied a
whole page-padded image (ADR-034 amendment 2026-11-23, `reports/05-vm.md`).

- **VMI-1** A module's active data segments are copied into linear memory, byte for byte, by fuel-metered code before `invoke` runs. The gas this costs is a function of the module alone: it is identical on every operating system, CPU architecture and execution tier, and never depends on a page size or on whether the host can map memory copy-on-write. A call whose `gas_limit` is below the instantiation cost plus what `invoke` runs fails `OutOfGas` and writes nothing; at or above it the call proceeds.

## Gaps

- The cost of a data segment is pinned by vectors, not by a formula in this
  section: it is wasmtime 48's fuel accounting of its own segment copy, and a
  wasmtime upgrade that changed it would be a gas-schedule change needing an
  ADR (the vectors would fail first).
- Gas for the rest of a call (instructions, host functions, memory) is
  specified by ADR-142 and, once it lands, `spec/10-vm-gas.md` (PR #643).
