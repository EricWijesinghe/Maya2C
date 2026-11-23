# 2026-11-15 — PR #549, second review (ADR-142 amendment 2)

Branch `refreeze/vm-gas-caps`, on top of f6652709. Decision: Eric
2026-11-14 MCQ ("cap + price now"). Every cargo command ran through
`cargo-slot.sh` on this PC (Windows 11, x86-64, shared with other agents).
Output below is copied from the runs, trimmed to the result lines.

## Red first

New VM tests against the f6652709 VM source (`runtime.rs`, `shape.rs`,
`gas.rs` restored, plus a one-line `ARITY_FUEL` shim so the tests compile):

```
test a_module_with_a_wide_function_type_is_refused_at_deploy ... FAILED
test a_call_that_recurses_forever_reports_its_whole_limit ... FAILED
test a_failed_call_reports_the_gas_it_used ... FAILED
test a_failed_call_reports_the_memory_it_touched_on_top_of_its_limit ... FAILED
test result: FAILED. 6 passed; 4 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
```

New node tests against the f6652709 node source (`mempool.rs`, `db.rs`,
`sealed_exec.rs`, `vm_exec.rs` restored; new VM kept):

```
test a_deployment_above_the_deploy_price_is_refused_at_admission ... FAILED
test one_sender_gets_one_failed_contract_trial_per_block ... FAILED
test a_call_below_its_upfront_price_still_counts_its_code_read_and_scan ... FAILED
test a_replayed_old_transaction_does_not_shut_its_sender_out ... ok   (regression guard)
test result: FAILED. 12 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.95s
```

## Green

`cargo test -p maya-vm` (debug):

```
lib 13 passed · cache 6 · fuel_calibration 1 · gas_adversarial_calibration 0 (5 ignored, timing)
gas_hardening 10 · gas_price_calibration 4 · gas_pricing 7 · host_surface 6 · oracle_wide 2
private_state 1 · property 13 · resource 10 · tensor_gas 5 · tier_differential 0 (3 ignored on Windows)
token_swap 12 · trace_site 1 · vm_tests 31 · zkml_host 10
```

After the loop price: `--lib --test gas_hardening_tests --test gas_pricing_tests
--test vm_tests --test property_tests`: 13, 10, 7, 13, 31 passed.

`cargo test -p custom-l1-node`:

```
--lib                     545 passed; 0 failed; 1 ignored (the 64 MiB DAG cache, existing)
vm_gas_tests              15 passed
sealed_tests              23 passed
block_bytes_tests         10 passed
vault_tests                8 passed
contract_tests            12 passed
exploit_replays           11 passed
nft_game_on_node_tests     1 passed
vm_gas_timing_tests       10 passed (--include-ignored)
```

The first `vm_gas_tests` run after the arity price failed only
`the_spec_vectors_hold`: 10 golden values moved by the arity price (5,000 for
the `(i32) -> (i64)` export type alone, more with host imports), one by the
uncapped memory (`golden-out-of-gas-uses-its-limit`: 1,006,000), and the
three new golden vectors needed their values. Updated from that output and
re-run green.

`cargo xtask spec-coverage --strict`: `69 rules, 212 vectors; 66 consensus
rules covered, 0 gaps`. `cargo xtask claims-check`: `97 public files scanned,
0 unauthorised claims`.

## Compile calibration (release)

`cargo test --release -p maya-vm --test gas_adversarial_calibration_tests --
--ignored --nocapture --test-threads 1 shapes_at_the_limits`, three runs; the
table is in ADR-142 amendment 2. The first run, at `ARITY_FUEL` 2,500 for
every carried value:

```
ADV at-limit loops [8]->[8] x 2000, 3 fns                 6137 B    726.784 ms price   142936650   5.085 ns/fuel
ADV at-limit worst 5.085 ns/fuel (bound 1.5)
test result: FAILED.
```

Scratch series (file not committed), `loop (type [k]->[])` in one function:

```
ARITY loop [8]->[0] x500 x1     26.986 ms   ARITY loop [16]->[0] x500 x1     44.402 ms
ARITY loop [8]->[0] x1000 x1    93.498 ms   ARITY loop [16]->[0] x1000 x1   165.775 ms
ARITY loop [8]->[0] x2000 x1   416.513 ms   ARITY loop [16]->[0] x2000 x1   823.265 ms
```

With `LOOP_ARITY_FUEL` = 30,000 (runs 2 and 3):

```
ADV at-limit loops [8]->[8] x 2000, 3 fns             price  1022936650 over the deploy price, refused
ADV at-limit loops [8]->[8] x 416                         1385 B     31.525 ms price   212958250   0.148 ns/fuel
ADV at-limit loops [8]->[8] with a back edge x 393        2888 B     35.025 ms price   213694600   0.164 ns/fuel
ADV at-limit worst 1.495 ns/fuel (bound 1.5)     (run 2: "loops at the per-function cap", 0.617 in run 1)
ADV at-limit worst 1.325 ns/fuel (bound 1.5)     (run 3)
ADV contract nft_game.wasm            2306 B      5.778 ms price     5508260   1.049 ns/fuel
ADV contract token_swap.wasm          2487 B      6.699 ms price     6303420   1.063 ns/fuel
```

Run 1 was quieter; runs 2 and 3 ran beside other agents' builds. Best of the
three is under 1 ns/fuel on every shape; the highest single run is 1.495.

Instantiation (`calibrate_instantiation_shapes`, release, warm call against
its warm price):

```
ADV instantiate empty                                  54 B      0.093 ms warm price      50054   1.856 ns/fuel
ADV instantiate 100k immutable globals             500061 B      0.544 ms warm price     550061   0.989 ns/fuel
ADV instantiate 80k mutable globals                400061 B      0.572 ms warm price     450061   1.272 ns/fuel
ADV instantiate 70k one-byte data segments         420061 B      0.058 ms warm price     470061   0.123 ns/fuel
ADV instantiate 1k passive 400 B data segments     403060 B      0.052 ms warm price     453060   0.116 ns/fuel
```

Under a millisecond each; mutable globals 1.27 ns/fuel against a warm price
that is mostly the call base. Recorded, not repriced.

## Worst-case blocks (dev profile, loaded machine)

```
TIMING host calls (get_balance_u256)          422.2 ms   10 txs  gas   250000000   1.689 ns/gas
TIMING 133 calls touching 16 MiB each         371.3 ms  133 txs  gas   248687124   1.493 ns/gas
TIMING cold calls (compile)                   365.7 ms   10 txs  gas   242421000   1.509 ns/gas
TIMING deploys (compile)                      354.3 ms   10 txs  gas   242421000   1.461 ns/gas
TIMING 19740 writes, then 200 warm calls      258.7 ms  201 txs  gas   325000000   0.796 ns/gas
TIMING selection, 8 failing max-gas calls      87.7 ms
TIMING contracts root fold,  400000 entries     844.9 ms
```

The fold (BACKLOG G2) was 533 ms on the first review's run; 845 ms here, on
a busier machine. It is still the genesis blocker the ADR names.

## NOT VERIFIED

- aarch64, Pulley, other x86-64 ISAs: CI job `vm-gas-aarch64` (added on this
  branch) is the first replay; it has not run yet.
- Release timing of whole blocks; the release compile figures above are per
  module, on one machine.
- Coverage `--verify-targets`, clippy ratchet: left to CI.
