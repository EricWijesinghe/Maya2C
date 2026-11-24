# 2026-11-22 — PR #549, fifth push: deployment scans counted, GAS-9, R-6 restated

Branch `refreeze/vm-gas-caps`, on top of 7ca43a0b with origin/master merged in.
Windows 11 Pro 29708, Intel Core Ultra 9 275HX, `nightly-2026-07-15`, every
cargo command through `cargo-slot.sh`.

## What changed

- `deploy_contract`: refused on `deploy_fuel(len)` before the shape scan; any
  refusal after the scan (shape, `BlockGasExhausted`, `GasAboveCap`) counts
  `deploy_fuel(len)`, as a cold call does (R-5).
- `trial_gas_bound`: a deployment is bounded by what the block has left, so
  block selection no longer scans every deployment it considers.
- GAS-9: the selection budget (`BUILDER_GAS_CAPS`, `BUILDER_GAS_SLACK`, one
  failure per sender) is consensus under DAG-BFT; four `builder_select`
  vectors.
- `maya_vm::config` names the build's target triple (`crates/vm/build.rs`),
  so Cranelift no longer infers ISA flags from the host CPU.
- R-6 restated in spec Gaps, BACKLOG G4 and ADR-142 amendment 6: the
  condition is one release binary (one target triple) for every validator.
- `#![allow(clippy::unwrap_used, clippy::expect_used)]` on the `gas.rs` and
  `shape.rs` test modules (workspace convention; 5 clippy errors before).

## Red

New deployment tests against the previous `vm_exec.rs`
(`cargo test -p custom-l1-node --lib state::vm_exec::tests`):

```
test state::vm_exec::tests::the_builder_bounds_a_deployment_by_what_the_block_has_left_without_its_scan ... FAILED
test state::vm_exec::tests::a_deployment_whose_length_price_does_not_fit_is_refused_before_the_scan ... FAILED
test state::vm_exec::tests::a_deployment_whose_shape_is_refused_counts_the_length_price ... FAILED
test state::vm_exec::tests::a_deployment_refused_after_its_shape_scan_counts_the_length_price ... FAILED
test result: FAILED. 5 passed; 4 failed; 0 ignored; 0 measured; 546 filtered out; finished in 2.51s
```

Target-pin test with `deterministic_config` switched back to host inference
(`cargo test -p maya-vm --lib named_target`): `assertion left != right
failed` (identical precompiled bytes).

## Green

```
cargo test -p maya-vm --lib config
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out; finished in 0.07s
cargo test -p custom-l1-node --lib state::
test result: ok. 133 passed; 0 failed; 0 ignored; 0 measured; 422 filtered out; finished in 1.43s
cargo test -p custom-l1-node --test vm_gas_tests
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.14s
cargo test -p custom-l1-node --test governance_tests block_gas_cap
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 32 filtered out; finished in 1.35s
cargo test -p maya-vm --test gas_hardening_tests --test gas_pricing_tests
test result: ok. 10 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
cargo xtask spec-coverage --strict
GAS-9    10-vm-gas.md            2    2  covered
71 rules, 217 vectors; 68 consensus rules covered, 0 gaps:
cargo clippy -p maya-vm --all-targets -- -A clippy::pedantic -A clippy::unwrap_used -D warnings   (clean)
cargo clippy -p custom-l1-node --all-targets -- -A clippy::pedantic -A clippy::unwrap_used -D warnings   (clean)
cargo clippy -p custom-l1-node --lib --bins -- -D clippy::unwrap_used   (clean)
```

`vm_gas_tests` includes the R-2 tests in both paths:
`a_pooled_failing_call_at_the_cap_leaves_honest_calls_in_every_produced_block`
and `a_pooled_failing_call_at_the_cap_leaves_honest_calls_in_every_bft_block`.

Stack depth with the target named, dev, Windows x86-64:
`R-6 x86_64 deepest completing depth 10901, gas 559418 at limit 250000000; depth 10902 overflows`
(the same as with inferred flags).

## Not verified

- aarch64 depth: CI `vm-gas-aarch64` prints it from this push.
- The shipped `maya2c-node` binaries' own depths (fat LTO, production).
- `spec-coverage` ran with a local-only `chrono` line in `xtask/Cargo.toml`:
  `xtask/src/devlog.rs` uses chrono and master's `xtask/Cargo.toml` does not
  list it. Not committed here.
- Coverage: left to CI.
