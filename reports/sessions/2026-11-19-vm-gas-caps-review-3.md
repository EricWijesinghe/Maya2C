# 2026-11-19 — PR #549 HIGH findings: failed builder work, compile shapes re-measured

ADR-142 amendment 3 (`adr-claims.txt`: 142c), branch `refreeze/vm-gas-caps`.
Under Eric 2026-11-17 pre-approval (re-freeze bundle 3).

Machine: Intel Core Ultra 9 275HX, 24 cores, 31 GB, Windows 11 Pro Insider
Preview build 29708, shared with other agents' builds. rustc 1.99.0-nightly
(da80ed070 2026-07-14), wasmtime 48.0.8. Every cargo command ran through
`cargo-slot.sh`.

## Finding 1: failed contract work on its own builder count

### Red, before the selection change

`cargo test -p custom-l1-node --test vm_gas_tests` with the new and restated
tests, `select_applicable_within` unchanged:

```
thread 'one_sender_gets_one_failed_contract_trial_per_block' (20184) panicked at crates\node\tests\vm_gas_tests.rs:555:5:
thread 'failed_work_past_the_builder_budget_stops_contract_trials' (61536) panicked at crates\node\tests\vm_gas_tests.rs:526:5:
thread 'a_failed_call_spends_the_gas_it_used_not_the_gas_it_named' (71424) panicked at crates\node\tests\vm_gas_tests.rs:681:5:
thread 'a_call_below_its_upfront_price_still_counts_its_code_read_and_scan' (66852) panicked at crates\node\tests\vm_gas_tests.rs:619:5:
failures:
    a_call_below_its_upfront_price_still_counts_its_code_read_and_scan
    a_deployment_that_fails_validation_still_spends_its_compile
    a_failed_call_spends_the_gas_it_used_not_the_gas_it_named
    a_failing_call_at_the_cap_does_not_crowd_out_a_call_that_applies
    failed_work_past_the_builder_budget_stops_contract_trials
    one_sender_gets_one_failed_contract_trial_per_block
test result: FAILED. 10 passed; 6 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.17s
```

Five failed on the shared counter (the honest call after a failure at the cap
was left out). `failed_work_past_the_builder_budget_stops_contract_trials`
failed its own precondition: a 1 M call fits inside the 1,536,000 slack. Its
honest limit was raised to 2 M before the fix.

### Green, after the fix

`cargo test -p custom-l1-node --test vm_gas_tests --test sealed_tests --test contract_tests --test block_bytes_tests`:

```
Running tests\block_bytes_tests.rs
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 22.58s
Running tests\contract_tests.rs
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.61s
Running tests\sealed_tests.rs
test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.78s
Running tests\vm_gas_tests.rs
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.69s
```

`cargo clippy -p custom-l1-node --all-targets -- -A clippy::pedantic -A clippy::unwrap_used -D warnings`:
`Finished 'dev' profile [unoptimized + debuginfo] target(s) in 3m 37s`, exit 0, no warnings.

### Builder selection timing (dev)

`cargo test -p custom-l1-node --test vm_gas_timing_tests -- --ignored --nocapture --test-threads 1 building_a_block_of_failing`, 2026-11-19 01:56 KST:

```
TIMING selection, 8 failing max-gas calls      56.9 ms
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 3.06s
```

Eight out-of-gas loop calls, each reporting a quarter cap, so 500 M gas of
failed work in all. The loop is cheap guest compute, so this shows the budget
is enforced. It does not test the 1 ns rule.

## Finding 2: compile shapes at the arity bound, release

`cargo test --release -p maya-vm --test gas_adversarial_calibration_tests -- --ignored --nocapture --test-threads 1 shapes_at_the_limits`.
Each figure is the best of three compiles (`RUNS`). Run 1 was at d25ff726.
Runs 2 and 3 added two attribution shapes to the test, with the VM unchanged.

### Run 1 (ended before 00:42 KST)

```
ADV at-limit blocks [8]->[8] x 2000, 3 fns                6137 B     64.304 ms price   142936650   0.450 ns/fuel
ADV at-limit loops [8]->[8] x 2000, 3 fns             price  1022936650 over the deploy price, refused
ADV at-limit ifs [8]->[8] x 1000, 3 fns                  12166 B    160.357 ms price   205774700   0.779 ns/fuel
ADV at-limit blocks []->[16] x 2000, 3 fns              102115 B    229.131 ms price   186126750   1.231 ns/fuel
ADV at-limit br_if carrying 8 x 1999, 3 fns              24193 B    159.857 ms price   191431850   0.835 ns/fuel
ADV at-limit loops [1]->[1] x 2000, 3 fns                 6121 B    139.630 ms price   182929450   0.763 ns/fuel
ADV at-limit loops [8]->[8] x 416                         1385 B     25.218 ms price   212958250   0.118 ns/fuel
ADV at-limit loops [8]->[8] with a back edge x 393        2888 B     30.868 ms price   213694600   0.144 ns/fuel
ADV at-limit nested loops [8]->[8] x 416                  1385 B     38.647 ms price   212958250   0.181 ns/fuel
ADV at-limit calls [8]->[8] x 4424                        9011 B     85.921 ms price   216746950   0.396 ns/fuel
ADV at-limit calls [16]->[] x 3318                      112937 B     51.214 ms price   210425650   0.243 ns/fuel
ADV at-limit br_table over 16 typed labels x 10000       30355 B      4.143 ms price    17534750   0.236 ns/fuel
ADV at-limit 256 live across 10000 calls                 20895 B     61.999 ms price    89702920   0.691 ns/fuel
ADV at-limit 256 live across 1000 br_if                   7883 B     25.780 ms price    63722520   0.405 ns/fuel
ADV at-limit 2048 live across 10000 calls                26271 B     61.200 ms price    92122120   0.664 ns/fuel
ADV at-limit 2048 live across 1000 br_if                 13259 B     34.450 ms price    66141720   0.521 ns/fuel
ADV at-limit empty functions, exported                   21693 B    206.617 ms price   229936850   0.899 ns/fuel
ADV at-limit max locals                                   1433 B     20.867 ms price    60510850   0.345 ns/fuel
ADV at-limit max locals, used in a loop                 118011 B      5.319 ms price    59736550   0.089 ns/fuel
ADV at-limit ifs at the per-function cap                 36046 B    138.757 ms price   196645700   0.706 ns/fuel
ADV at-limit loops at the per-function cap               18046 B    161.514 ms price   188545700   0.857 ns/fuel
ADV at-limit br_if at the per-function cap               21046 B     48.889 ms price   189895700   0.257 ns/fuel
ADV at-limit nested blocks at the cap                    18046 B      5.372 ms price   188545700   0.028 ns/fuel
ADV at-limit calls at the per-function cap               20037 B     52.734 ms price    89191650   0.591 ns/fuel
ADV at-limit br_table                                    10044 B      0.982 ms price     4754800   0.206 ns/fuel
ADV at-limit worst 1.231 ns/fuel (bound 1.5)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 6.58s
```

### Run 2 (started 00:42:39 KST)

```
ADV at-limit blocks [8]->[8] x 2000, 3 fns                6137 B     59.705 ms price   142936650   0.418 ns/fuel
ADV at-limit loops [8]->[8] x 2000, 3 fns             price  1022936650 over the deploy price, refused
ADV at-limit ifs [8]->[8] x 1000, 3 fns                  12166 B    114.261 ms price   205774700   0.555 ns/fuel
ADV at-limit blocks []->[16] x 2000, 3 fns              102115 B    143.411 ms price   186126750   0.771 ns/fuel
ADV at-limit 16 pushed, dropped x 2000, no block        288127 B     12.279 ms price   130082150   0.094 ns/fuel
ADV at-limit blocks []->[8] x 2000, 3 fns                54126 B     91.044 ms price   124531700   0.731 ns/fuel
ADV at-limit br_if carrying 8 x 1999, 3 fns              24193 B    135.785 ms price   191431850   0.709 ns/fuel
ADV at-limit loops [1]->[1] x 2000, 3 fns                 6121 B     94.538 ms price   182929450   0.517 ns/fuel
ADV at-limit loops [8]->[8] x 416                         1385 B     20.353 ms price   212958250   0.096 ns/fuel
ADV at-limit loops [8]->[8] with a back edge x 393        2888 B     20.555 ms price   213694600   0.096 ns/fuel
ADV at-limit nested loops [8]->[8] x 416                  1385 B     15.387 ms price   212958250   0.072 ns/fuel
ADV at-limit calls [8]->[8] x 4424                        9011 B     61.008 ms price   216746950   0.281 ns/fuel
ADV at-limit calls [16]->[] x 3318                      112937 B     39.658 ms price   210425650   0.188 ns/fuel
ADV at-limit br_table over 16 typed labels x 10000       30355 B      3.672 ms price    17534750   0.209 ns/fuel
ADV at-limit 256 live across 10000 calls                 20895 B     51.288 ms price    89702920   0.572 ns/fuel
ADV at-limit 256 live across 1000 br_if                   7883 B     18.098 ms price    63722520   0.284 ns/fuel
ADV at-limit 2048 live across 10000 calls                26271 B     46.173 ms price    92122120   0.501 ns/fuel
ADV at-limit 2048 live across 1000 br_if                 13259 B     25.376 ms price    66141720   0.384 ns/fuel
ADV at-limit empty functions, exported                   21693 B    150.077 ms price   229936850   0.653 ns/fuel
ADV at-limit max locals                                   1433 B     16.954 ms price    60510850   0.280 ns/fuel
ADV at-limit max locals, used in a loop                 118011 B      4.428 ms price    59736550   0.074 ns/fuel
ADV at-limit ifs at the per-function cap                 36046 B    114.349 ms price   196645700   0.581 ns/fuel
ADV at-limit loops at the per-function cap               18046 B    143.948 ms price   188545700   0.763 ns/fuel
ADV at-limit br_if at the per-function cap               21046 B     45.869 ms price   189895700   0.242 ns/fuel
ADV at-limit nested blocks at the cap                    18046 B      4.296 ms price   188545700   0.023 ns/fuel
ADV at-limit calls at the per-function cap               20037 B     45.916 ms price    89191650   0.515 ns/fuel
ADV at-limit br_table                                    10044 B      0.733 ms price     4754800   0.154 ns/fuel
ADV at-limit worst 0.771 ns/fuel (bound 1.5)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 5.10s
```

### Run 3 (started 01:11:17 KST)

```
ADV at-limit blocks [8]->[8] x 2000, 3 fns                6137 B     53.896 ms price   142936650   0.377 ns/fuel
ADV at-limit loops [8]->[8] x 2000, 3 fns             price  1022936650 over the deploy price, refused
ADV at-limit ifs [8]->[8] x 1000, 3 fns                  12166 B    122.000 ms price   205774700   0.593 ns/fuel
ADV at-limit blocks []->[16] x 2000, 3 fns              102115 B    142.946 ms price   186126750   0.768 ns/fuel
ADV at-limit 16 pushed, dropped x 2000, no block        288127 B     12.721 ms price   130082150   0.098 ns/fuel
ADV at-limit blocks []->[8] x 2000, 3 fns                54126 B     90.337 ms price   124531700   0.725 ns/fuel
ADV at-limit br_if carrying 8 x 1999, 3 fns              24193 B    125.627 ms price   191431850   0.656 ns/fuel
ADV at-limit loops [1]->[1] x 2000, 3 fns                 6121 B     83.113 ms price   182929450   0.454 ns/fuel
ADV at-limit loops [8]->[8] x 416                         1385 B     18.957 ms price   212958250   0.089 ns/fuel
ADV at-limit loops [8]->[8] with a back edge x 393        2888 B     21.183 ms price   213694600   0.099 ns/fuel
ADV at-limit nested loops [8]->[8] x 416                  1385 B     19.638 ms price   212958250   0.092 ns/fuel
ADV at-limit calls [8]->[8] x 4424                        9011 B     49.976 ms price   216746950   0.231 ns/fuel
ADV at-limit calls [16]->[] x 3318                      112937 B     30.956 ms price   210425650   0.147 ns/fuel
ADV at-limit br_table over 16 typed labels x 10000       30355 B      3.066 ms price    17534750   0.175 ns/fuel
ADV at-limit 256 live across 10000 calls                 20895 B     37.629 ms price    89702920   0.419 ns/fuel
ADV at-limit 256 live across 1000 br_if                   7883 B     18.291 ms price    63722520   0.287 ns/fuel
ADV at-limit 2048 live across 10000 calls                26271 B     39.954 ms price    92122120   0.434 ns/fuel
ADV at-limit 2048 live across 1000 br_if                 13259 B     21.455 ms price    66141720   0.324 ns/fuel
ADV at-limit empty functions, exported                   21693 B    130.252 ms price   229936850   0.566 ns/fuel
ADV at-limit max locals                                   1433 B     14.645 ms price    60510850   0.242 ns/fuel
ADV at-limit max locals, used in a loop                 118011 B      3.660 ms price    59736550   0.061 ns/fuel
ADV at-limit ifs at the per-function cap                 36046 B    110.556 ms price   196645700   0.562 ns/fuel
ADV at-limit loops at the per-function cap               18046 B     98.426 ms price   188545700   0.522 ns/fuel
ADV at-limit br_if at the per-function cap               21046 B     40.190 ms price   189895700   0.212 ns/fuel
ADV at-limit nested blocks at the cap                    18046 B      3.986 ms price   188545700   0.021 ns/fuel
ADV at-limit calls at the per-function cap               20037 B     41.493 ms price    89191650   0.465 ns/fuel
ADV at-limit br_table                                    10044 B      0.718 ms price     4754800   0.151 ns/fuel
ADV at-limit worst 0.768 ns/fuel (bound 1.5)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 4.54s
```

Outcome: the prices are unchanged, and ADR-142's claim now gives the real
range. In runs 2 and 3 every shape is at or under 1 ns/fuel. Run 1 is 1.231
on the same binary and shape, which is machine load.

## Not verified

- Workspace-wide clippy and `cargo xtask coverage --verify-targets`: left to CI (AGENT-RULES).
- The node library's own unit tests (`cargo test -p custom-l1-node --lib`) were not run locally.
- No unloaded machine was available for calibration. aarch64, Pulley and other x86-64 CPUs were not timed.
- `cargo xtask spec-coverage --strict`: see the PR body for its result.
