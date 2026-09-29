# ADR-034: The consensus VM rejects reference types and typed function references

**Status:** Accepted (2026-09-29). A validation-rule change in the consensus VM.
**Date:** 2026-09-29

## Context

On 2026-09-29, RustSec published two wasmtime advisories against the engine
this chain pins (`wasmtime =48.0.1`):

- **RUSTSEC-2026-0315.** `call_ref` and exception `catch` can drop fuel
  accounting, which leads to exponential fuel amplification. In this chain,
  fuel is gas.
- **RUSTSEC-2026-0316.** Component-model record lifting can allocate past the
  hostcall fuel limit.

Both are fixed in 48.0.3. `crates/vm/src/config.rs` records why a wasmtime
bump is not a routine update: fuel accounting is not guaranteed stable across
releases, so validators on two versions can disagree about gas, and that
splits the chain.

The obvious assumption was that neither bug could reach a contract.
`config.rs` has always listed "`wasm_reference_types` off" among its
determinism settings, and the crate is built with `default-features = false`,
which drops `gc` and the component model. A test written to prove it showed
otherwise for the first advisory:

- a module using `call_ref` **validated**. wasmtime compiles the named setters
  `wasm_reference_types` and `wasm_function_references` only with the `gc`
  feature. Without it, the proposals kept their default, which is *on*, and
  nothing in the configuration could say otherwise. The table in `config.rs`
  described an intention the code did not carry out.
- exception handling was rejected (the proposal is not enabled);
- a component was rejected (the component model is compiled out).

So any contract could reach RUSTSEC-2026-0315: a denial-of-service in which a
call runs far more computation than its gas paid for.

## Decision

`deterministic_config` turns both proposals off through the generic
`Config::wasm_features`, which does not need `gc`:

```rust
config.wasm_features(
    WasmFeatures::REFERENCE_TYPES | WasmFeatures::FUNCTION_REFERENCES,
    false,
);
```

- The same review found four more proposals that wasmtime 48 enables by
  default and that the determinism table never listed: **tail calls,
  extended constant expressions, multiple memories and 64-bit memories**. None
  has a known gas bug, and no contract built by `scripts/build-contract.sh`
  uses them. All four are turned off too, and memory64's absence keeps
  `MAX_MEMORY_PAGES` the only memory ceiling. A proposal comes back only when a
  contract needs it and a fuel test covers it.
- The config digest covers both new settings. Every module-cache key changes,
  so no module compiled under the old configuration is served afterwards.
- Seven tests in `crates/vm/tests/vm_tests.rs` pin the reasons: a `call_ref`
  module, an exception-handling module, a component, and a module using each
  of the four default-on proposals are all rejected by
  `Vm::validate`.
- `deny.toml` ignores both advisories and cites those tests. An ignore is
  justified only while the tests pass.
- wasmtime stays at 48.0.1. Moving to 48.0.3 or later is scheduled as a
  deliberate fork, with fuel calibration re-run and an activation height.

## Consequences

- **This is a consensus rule change.** A module using reference types or
  function references was deployable before and is refused now. A node on the
  old rule would accept such a deployment and a node on the new rule would
  refuse it.
- **Why it takes effect without an activation height.** maya-testnet-1's genesis
  is dated 2026-09-29, the day of this change. Its only validator runs this
  code, and its public endpoints were not yet published, so no outside
  deployment could have happened. Before mainnet, a change of this kind needs
  an activation height, per Standing Order 3 and invariant 24's spirit: every
  node must compute the same state root.
- Contracts built by `scripts/build-contract.sh` are unaffected. A fresh build
  of `contracts/token-swap` with nightly-2026-07-15 passes all 12
  `token_swap_tests` under the new rule.
- The lesson: a setting the code cannot express is not a setting. The
  determinism table is now backed by a test for every proposal it names.
