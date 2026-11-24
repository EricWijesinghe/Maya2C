# 2026-11-21 — PR #549, fourth push: re-freeze review R-2, R-5, R-6, R-10 and GAS-5

Branch `refreeze/vm-gas-caps`. ADR-142 amendment 4. Every figure below is
from a command run in this session through the build slot
(`D:/Temp/claude-locks/cargo-slot.sh`), on Intel Core Ultra 9 275HX, Windows
11 Pro 29708, nightly-2026-07-15, shared with other agents' builds.

## What was merged in

- `origin/master` twice (last at f2305189). One conflict: `docs/adr/README.md`
  (ADR-140 row and ADR-142 row, both kept).
- `origin/refreeze/validity-params-schedule` (#575), which is ahead of #549 in
  the bundle's merge order. Conflicts: `crates/governance/src/params.rs` (both
  blocks kept), `Cargo.lock` (master's `time` kept). Red, captured:

```
353 ~             ParameterKey::VmMaxTxGas | ParameterKey::VmMaxBlockGas => todo!(),
For more information about this error, try `rustc --explain E0004`.
error: could not compile `maya-governance` (lib) due to 1 previous error
```

  Resolved as #626 did: tags 17 and 18 are validity rules.

## Reds captured before each fix

```
test state::vm_exec::tests::a_cold_call_below_the_length_price_is_refused_without_the_shape_scan ... FAILED
  left: Err(Vm("function locals of 1025 bytes exceeds the limit of 1024"))
 right: Err(Vm("out of gas: limit was 62149"))

test a_waiting_block_gas_cap_out_of_range_at_promotion_is_dropped ... FAILED
test a_passed_block_gas_cap_binds_only_after_the_validity_rule_lead ... FAILED
(tags 17 and 18 classified as immediate for the red run)

test state::vm_exec::tests::a_cold_call_refused_after_the_shape_scan_counts_the_length_price ... FAILED
  left: 50054
 right: 74300
```

The R-2 and R-10 tests passed on the first run (verification of existing
code, not fixes):

```
test a_block_spends_its_cap_on_plaintext_calls_and_the_sealed_budget_on_reveals ... ok
test a_pooled_failing_call_at_the_cap_leaves_honest_calls_in_every_bft_block ... ok
test a_pooled_failing_call_at_the_cap_leaves_honest_calls_in_every_produced_block ... ok
```

## Measurements

`cargo test --release -p maya-vm --test gas_adversarial_calibration_tests --test gas_hardening_tests -- --ignored --nocapture --test-threads 1 calibrate_the_shape_scan measure_the_deepest_recursion`

```
SCAN one body of nop                   524261 B   640.900 us  1.222 ns/B  1.116 ns per unit of 50,000 + len  0.0027 ns per unit of deploy_fuel
SCAN one body of i32.const 0; drop     524260 B   457.000 us  0.872 ns/B  0.796 ns per unit of 50,000 + len  0.0019 ns per unit of deploy_fuel
SCAN one br_table, all labels          524255 B   609.300 us  1.162 ns/B  1.061 ns per unit of 50,000 + len  0.0026 ns per unit of deploy_fuel
SCAN one local entry per local         513525 B   354.800 us  0.691 ns/B  0.630 ns per unit of 50,000 + len  0.0015 ns per unit of deploy_fuel
SCAN 10,000 empty functions             40034 B    67.900 us  1.696 ns/B  0.754 ns per unit of 50,000 + len  0.0038 ns per unit of deploy_fuel
R-6 x86_64 deepest completing depth 10915, gas 559558 at limit 250000000; depth 10916 overflows
```

Dev profile, same source and machine:
`cargo test -p maya-vm --test gas_hardening_tests -- --ignored --nocapture measure_the_deepest_recursion`

```
R-6 x86_64 deepest completing depth 10901, gas 559418 at limit 250000000; depth 10902 overflows
```

A golden vector at the release boundary was written and failed in the dev
replay, so it was replaced by a 5,000-deep vector:

```
"golden-recursion-at-the-x86-64-stack-boundary-completes: error Some(\"Trap\"), expected null",
"golden-recursion-5000-deep-completes-on-every-build: value 499958, expected \"0\"",   (value read, then pinned)
```

499,958 matches the slope of the two boundary figures (10 gas a level) less
450 for the one byte shorter constant.

## Final runs on the merged tree

`cargo test --no-fail-fast -p custom-l1-node --lib --test vm_gas_tests --test sealed_tests --test governance_tests --test contract_tests --test upgrade_schedule_tests --test council_pause_tests`

```
lib                     550 passed; 0 failed; 1 ignored
contract_tests           12 passed; 0 failed
council_pause_tests       5 passed; 0 failed
governance_tests         34 passed; 0 failed
sealed_tests             24 passed; 0 failed
upgrade_schedule_tests    7 passed; 0 failed
vm_gas_tests             18 passed; 0 failed
```

`cargo test --no-fail-fast -p maya-vm -p maya-governance`: 21 targets, 173
passed, 0 failed (ignored timing tests not run there).

`cargo clippy -p custom-l1-node -p maya-vm -p maya-governance --all-targets -- -A clippy::pedantic -A clippy::unwrap_used -D warnings`:
`Finished dev profile`, no warnings, no errors.

`cargo fmt -p custom-l1-node -p maya-vm -p maya-governance -- --check`: clean
after `cargo fmt`.

`cargo xtask spec-coverage --strict`: `70 rules, 213 vectors; 67 consensus
rules covered, 0 gaps`. **`xtask` does not compile on origin/master
(f2305189):** `xtask/src/devlog.rs` (from #539) still calls `chrono`, which
#632 removed (E0433 at lines 84 and 212). The run above used a local,
uncommitted two-line stub of `devlog.rs`; nothing of it is in this PR.

## Not verified

- aarch64: nothing run here; the `vm-gas-aarch64` CI job is the replay.
- The `ci` cargo profile CI uses (dev was used locally).
- `coverage --verify-targets`: left to CI.
- Workspace-wide clippy and tests.
