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

## Amendment 2026-11-23: copy-on-write memory images off

**Status:** Proposed. Written by Claude under Eric's blanket "I approve
everything, proceed" (Eric 2026-11-14). That text does not name gas-schedule
changes, so it is cited as the reason the work was done, not as approval of
this change. It moves to Accepted when two human reviewers and the
rust-reviewer and security-reviewer passes are recorded on PR #674.

**Finding.** The V3-P3 tier work explained the Windows "Pulley charges
61,440 more fuel" result from 2026-09-27, and it was not about Pulley.
wasmtime 48 turns active data segments into a memory image padded to the
compiler's page size. Where the image can be mapped copy-on-write (Linux,
memfd), the segments cost nothing. Where it cannot (Windows), a startup
function copies the whole padded image, and that function is fuel-metered.
Measured with one 16-byte segment and nothing else
(`a_data_segment_costs_the_same_gas_on_every_host_and_tier`):

| Host | Cranelift | Pulley |
|---|---|---|
| Linux (WSL2 Ubuntu 24.04), old config | 2 | 2 |
| Windows 11, old config | 4,099 | 65,539 |
| Windows 11 and Linux, `memory_init_cow(false)` | 20 | 20 |

So a Cranelift node on Windows and a Cranelift node on Linux charged
different gas for every contract with a data segment (Rust-built contracts
normally have one; `token_swap` used 4,154 on Windows). A call whose
`gas_limit` falls between the two figures succeeds on one platform and runs
out of gas on the other, and only the success writes storage: a state-root
split between validators on different operating systems, which anyone can
trigger by choosing the gas limit.

**Decision.** `deterministic_config` calls `config.memory_init_cow(false)`.
Every host then copies exactly the segments' bytes through the same metered
code, so data-segment gas is a function of the module alone. It is in
`config_digest`, so every cached module is recompiled.

**Consequences.**

- Gas changes for every contract with a data segment: on Linux it rises by
  about the segments' byte count, on Windows it falls by about one page. It
  takes effect at mainnet genesis, with no activation height, because it
  lands before genesis.
- **Testnet operational step.** A testnet binary carrying this change must
  replace every validator's binary at once (stop all, upgrade all, start
  all), or the testnet is re-genesised. A mixed testnet (old and new
  binaries, or Linux and Windows nodes on the old binary) disagrees on
  exactly the calls above. The same line is in the PR body for the release
  notes.
- Instantiation copies segment bytes instead of mapping them. Contracts are
  capped at `MAX_MODULE_BYTES` = 512 KiB, so the copy is bounded.
- On master before this change `tier_differential_tests.rs` had 2 tests,
  both ignored on Windows. Both now run there and pass, and the file has 6:
  the hand corpus, `token_swap` (its wasm is committed under `target-contracts/`;
  the test now fails instead of skipping if it is missing), the fuzz corpus, the corpus file names, the pinned data
  segment and the spec vectors. The Pulley tier stays off for its speed
  (19x slower), not for a divergence.
- **Spec and vectors.** Rule VMI-1 in `spec/11-vm-instantiation.md`, with
  hand-written vectors in `spec/tests/vm_instantiation.json` (16-byte
  segment: 20 gas, ok at `gas_limit` 20, `OutOfGas` at 19), replayed on
  every tier by `the_vm_instantiation_vectors_hold_in_every_tier`.
- **Golden values, not only tier agreement.** `tests/tier_corpus/golden.txt`
  (fuzz corpus, 512 cells) and `hand_golden.txt` (hand corpus, 24 cells)
  pin what every tier on every host must produce. They are regenerated
  with `MAYA_VM_BLESS_TIER_GOLDEN=1` only for a deliberate gas-schedule or
  wasmtime change, which needs an ADR or amendment citing the old and new
  values. With `memory_init_cow(true)` restored on Linux, both tiers agree
  and the hand golden fails (`storage and events`: 186 against 204): the
  check this bug needed.
- Lesson: "deterministic" has to be tested across the operating systems
  validators run, not only across tiers on one machine. The pinned numbers
  above fail on any host that charges differently.
