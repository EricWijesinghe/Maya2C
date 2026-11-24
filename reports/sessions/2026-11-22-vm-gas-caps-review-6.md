# 2026-11-22: #549 / #647 review of ADR-142 amendment 6 (amendment 7)

Branch `refreeze/vm-gas-caps` (#549), then merged into
`needs-eric/vm-failed-call-fee` (#647, draft). Machine: Intel Core Ultra 9
275HX, Windows 11 Pro. Every cargo command through `cargo-slot.sh`.

## Findings, checked against the code

| Finding | Verdict | Change |
|---|---|---|
| ISA baseline: float rounding as libcalls, popcnt as a sequence; timing never measured | Source confirmed (cranelift-codegen-meta 0.135.8 x86 defaults false). Measured: float rounding is 0.52-0.67 ns/fuel under every configuration, including the host-inferred one the calibration used; worst 0.674 (baseline `f64.nearest`), about 211 ms at the 1.25 x bound, inside 1 s | x86-64-v2 flags pinned (`ISA_FLAGS`), hashed in the digest, CPU without them refused at engine build; no repricing |
| #647: per-call engine faults absorbed as failed calls | Confirmed (`classify` -> `Trap`, `Module::new` -> `InvalidModule`, `memory_grow_failed` -> -1) | `VmError::LocalFault` for `io::Error` / `OutOfMemory`; node maps it to `NodeError::Storage` |
| Selection takes a local fault as a transaction failure (GAS-9) | Confirmed, same cause | Same fix; `stage_transfer_or_undo` already passes `Storage` up |
| Suggested rule "not a `Trap` = local" | Rejected: a declared table fails instantiation with a plain error on every node; under that rule every validator would stop building while it is pooled | Positive, by-type rule instead |

## Red, then green

Red (fixes reverted: `is_local_fault` returning false, `ISA_FLAGS` empty),
`cargo test -p maya-vm --lib`:

```
test runtime::local_fault_tests::a_compile_failure_is_local_only_when_the_system_refused ... FAILED
test runtime::local_fault_tests::the_limiter_passes_up_only_a_refusal_by_the_system ... FAILED
test runtime::local_fault_tests::memory_refused_while_running_is_a_local_fault ... FAILED
test runtime::local_fault_tests::memory_refused_at_instantiation_is_a_local_fault ... FAILED
test config::digest_tests::the_engine_compiles_at_the_x86_64_v2_level_not_the_bare_baseline ... FAILED
test result: FAILED. 17 passed; 5 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

Green: `cargo test -p maya-vm --lib`: `22 passed; 0 failed; 1 ignored`.
`cargo test -p maya-vm` (all targets): every binary `ok`, 0 failed.
`cargo test -p custom-l1-node --test vm_gas_tests --test contract_tests --test governance_tests --test vm_gas_timing_tests`:
contract 12 passed, governance 34 passed, vm_gas 18 passed, timing 11 ignored.
`cargo test -p custom-l1-node --lib ... vm_exec`: `10 passed; 0 failed`.

## Timing

`cargo test --release -p maya-vm --lib config:: -- --include-ignored --nocapture --test-threads 1`:

```
ISA-TIMING i64.div_u (the calibrated compute)   ns/fuel: host-inferred 0.128  baseline 0.128  ISA_FLAGS 0.127
ISA-TIMING f64.floor                            ns/fuel: host-inferred 0.563  baseline 0.406  ISA_FLAGS 0.519
ISA-TIMING f64.ceil                             ns/fuel: host-inferred 0.550  baseline 0.411  ISA_FLAGS 0.526
ISA-TIMING f64.nearest                          ns/fuel: host-inferred 0.550  baseline 0.674  ISA_FLAGS 0.518
ISA-TIMING f64.trunc                            ns/fuel: host-inferred 0.549  baseline 0.632  ISA_FLAGS 0.516
ISA-TIMING f32.floor                            ns/fuel: host-inferred 0.553  baseline 0.421  ISA_FLAGS 0.525
ISA-TIMING f32.nearest                          ns/fuel: host-inferred 0.555  baseline 0.511  ISA_FLAGS 0.517
ISA-TIMING i64.popcnt                           ns/fuel: host-inferred 0.066  baseline 0.219  ISA_FLAGS 0.066
ISA-TIMING i32.popcnt                           ns/fuel: host-inferred 0.073  baseline 0.198  ISA_FLAGS 0.074
ISA-TIMING i64.clz                              ns/fuel: host-inferred 0.065  baseline 0.103  ISA_FLAGS 0.103
ISA-TIMING i64.ctz                              ns/fuel: host-inferred 0.066  baseline 0.089  ISA_FLAGS 0.088
```

Block level, dev profile, at x86-64-v2:

```
TIMING guest compute (f64.nearest)            142.3 ms    1 txs  gas   250000000   0.569 ns/gas
TIMING guest compute (i64.popcnt)              22.0 ms    1 txs  gas   250000000   0.088 ns/gas
TIMING guest compute (i64.div_u)               40.4 ms    1 txs  gas   250000000   0.162 ns/gas
```

## NOT VERIFIED

- `cargo xtask spec-coverage --strict`: xtask does not compile in this tree
  (`xtask/src/devlog.rs` uses `chrono`, which `xtask/Cargo.toml` does not
  declare; from master #539, not this branch).
- aarch64: `ISA_FLAGS` is empty there; nothing timed on aarch64.
- A node-level test with the engine's memory refused: `StateDB` holds its
  own engine with no injection hook; the mapping is pinned at `vm_failure`.
- Release node binaries were not timed; `gas_adversarial_calibration_tests`
  (compile shapes) not re-run: code generation flags do not enter the
  compile prices' shapes, but that was not measured.
- Workspace clippy ratchet and `coverage --verify-targets`: left to CI.
