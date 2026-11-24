# 2026-11-14 — ADR-142 review fixes (PR #549)

Branch `refreeze/vm-gas-caps`, on top of `202d73f0`. Eric 2026-11-14 MCQ
("cap + price now"); the PR #549 review's findings, fixed or recorded in the
ADR-142 amendment. Machine: the owner's Windows 11 PC, shared with other
agents' builds. Every command ran through `cargo-slot.sh`. Output below is
copied from the runs, trimmed to the lines that matter.

## Red, before the fixes

VM, against `202d73f0` (`cargo test -p maya-vm --test gas_hardening_tests`):

```
test a_call_cannot_hold_more_memory_than_its_gas_pays_for ... FAILED
  Ok(Outcome { output: [], gas_used: 74752, events: [] })
test a_call_pays_for_the_linear_memory_it_holds ... FAILED
  76554 vs 76554
test a_module_with_too_many_locals_is_refused_before_it_compiles ... FAILED
  Ok(())
test a_refused_host_call_ends_the_call ... FAILED
  oversized storage_read key: the call kept running after the refusal
test an_event_topic_over_the_cap_is_refused ... FAILED
  Ok(129590)
test result: FAILED. 0 passed; 5 failed
```

Node, against `202d73f0`'s implementation with only the new tests applied
(`cargo test -p custom-l1-node --lib --test vm_gas_tests --test sealed_tests
--no-fail-fast -- <new test names>`):

```
state::vm_exec::tests::lazy_storage_reads_exactly_what_the_whole_storage_load_did ... FAILED
  assertion `left == right` failed: round 0  left: 8  right: 0
a_plaintext_call_that_fills_the_block_gas_does_not_void_a_due_sealed_call ... FAILED
  the sealed call must run on its own budget
a_sealed_deployment_that_fails_validation_still_spends_its_compile ... FAILED
  the failed deployment's compile must have used the budget the second needed
a_sealed_call_over_the_sealed_budget_is_refused_before_it_runs ... FAILED
  the honest sealed call must run
a_deployment_that_fails_validation_still_spends_its_compile ... FAILED   1 kept
a_call_that_fails_while_the_block_is_built_still_spends_its_gas ... FAILED   1 kept
vm_gas_tests: 2 passed; 2 failed (the two that passed pin behaviour the old code already had)
```

The original PR's node tests were not re-run against `a0ee0ad6`: they name
APIs that did not exist there (`ParameterKey::VmMaxTxGas`,
`vm_exec::admit_call_gas`), so that red is a compile failure by construction.

## Measurements that set the prices

Adversarial compile shapes before the fix, release build
(`gas_adversarial_calibration_tests -- --ignored`, length-only price):

```
ADV empty functions            80037 B  20000 fn   1061.206 ms  price  36066650   29.423 ns/fuel
ADV empty functions, exported 128802 B  10000 fn   1202.008 ms  price  58010900   20.720 ns/fuel
ADV locals 200 x 50000          1633 B    200 fn    677.605 ms  price    784850  863.355 ns/fuel
ADV branches 1 x 40000        240037 B      1 fn  10441.875 ms  price 108066650   96.624 ns/fuel
ADV per-fn loops 1 x 1000       3035 B             23.328 ms   7686.2 ns/B
ADV per-fn branches 1 x 500     3035 B             10.020 ms   3301.5 ns/B
ADV per-fn calls 1 x 10000     20037 B             61.386 ms   3063.6 ns/B
ADV memory declared 256 pages  touch 959.2 us  3.21 us/page over empty, 6.57 ns/gas
```

The same shapes at the new limits and prices, release
(`shapes_at_the_limits_compile_inside_their_price`, `ADV_CONTRACTS` set to
the repo's built contracts):

```
ADV at-limit empty functions, exported    21693 B  202.880 ms price 229936850  0.882 ns/fuel
ADV at-limit max locals                    1433 B   27.236 ms price  60510850  0.450 ns/fuel
ADV at-limit ifs at the per-function cap  36046 B  168.118 ms price 196645700  0.855 ns/fuel
ADV at-limit loops at the per-function cap 18046 B 157.501 ms price 188545700  0.835 ns/fuel
ADV at-limit br_if at the per-function cap 21046 B  62.664 ms price 189895700  0.330 ns/fuel
ADV at-limit calls                         40037 B 124.200 ms price 178191650  0.697 ns/fuel
ADV at-limit worst 0.882 ns/fuel (bound 1.5)
ADV contract nft_game.wasm   2306 B 4.872 ms price 5260760 (length only 1087700) 0.926 ns/fuel
ADV contract token_swap.wasm 2487 B 4.243 ms price 5933420 (length only 1169150) 0.715 ns/fuel
ADV memory declared 256 pages touch 1464.1 us gas 1355080 ... 4.90 us/page over empty, 1.08 ns/gas
ADV memory grown 1 -> 256     touch 1540.9 us gas 1355080 ... 5.73 us/page over empty, 1.14 ns/gas
test result: ok. 1 passed
```

A 200,000-target `br_table` is refused by Cranelift itself
(`failed to compile`), so the at-limit list uses 10,000.

## Green

`cargo test -p maya-vm --no-fail-fast` (dev): 19 targets ok, among them
`gas_hardening_tests` 7 passed, `gas_pricing_tests` 7, `property_tests` 13,
`vm_tests` 31, lib 9; `tier_differential_tests` 3 ignored on Windows (the
existing Pulley FINDING), `gas_adversarial_calibration_tests` 4 ignored.

`cargo test -p custom-l1-node --lib --test vm_gas_tests --test sealed_tests
--test contract_tests --test exploit_replays --test oracle_tests --test
pruned_node_tests --test nft_game_on_node_tests --test block_bytes_tests
--test vm_gas_timing_tests --no-fail-fast -- --include-ignored --nocapture
--test-threads 1`:

```
lib                 546 passed
block_bytes_tests    10 passed
contract_tests       12 passed
exploit_replays      11 passed
nft_game_on_node      1 passed
oracle_tests         30 passed
pruned_node_tests    19 passed
sealed_tests         23 passed
vm_gas_tests         11 passed
vm_gas_timing_tests  10 passed (all ignored by default; run here with --include-ignored)
```

Timing output from that run (dev profile):

```
TIMING storage reads (cold price, RocksDB hits)   19.4 ms  10 txs  gas 250000000  0.078 ns/gas
TIMING 134 calls touching 16 MiB each           330.1 ms 134 txs  gas 249886952  1.321 ns/gas
TIMING guest compute (i64.div_u)                 39.0 ms   1 txs  gas 250000000  0.156 ns/gas
TIMING host calls (get_balance_u256)            233.2 ms  10 txs  gas 250000000  0.933 ns/gas
TIMING storage writes (4 KiB)                    10.1 ms  10 txs  gas 250000000  0.040 ns/gas
TIMING 19745 writes, then 200 warm calls        172.4 ms 201 txs  gas 325000000  0.530 ns/gas
TIMING deploys (compile)                        347.3 ms  10 txs  gas 241471000  1.438 ns/gas
TIMING cold calls (compile)                     294.4 ms  10 txs  gas 241471000  1.219 ns/gas
TIMING selection, 8 failing max-gas calls       162.9 ms
TIMING cold read 1 B values, no bloom  400000 keys, 1 MiB cache, direct I/O: hit 113.23 us absent 105.28 us
TIMING cold read 4 KiB values, no bloom 40000 keys, 1 MiB cache, direct I/O: hit 1438.51 us absent 1994.26 us
TIMING cold read 1 B values, bloom     400000 keys, 1 MiB cache, direct I/O: hit 809.77 us absent 7.93 us
TIMING contracts root fold,       0 entries   0.2 ms
TIMING contracts root fold,  100000 entries 128.0 ms
TIMING contracts root fold,  400000 entries 532.9 ms
```

An earlier run of the cold-read test measured 73.45 µs (hit) and 78.48 µs
(absent) for 1-byte values without a bloom filter, 579.76 µs with one. The
bloom filter was dropped from `open_tuned` on that evidence. The 4 KiB case
is the index block not fitting a 1 MiB cache; recorded as unpriced in
ADR-142.

`cargo test -p maya-governance`: 30 + 8 + 2 passed.
`cargo xtask spec-coverage --strict`: `69 rules, 205 vectors; 66 consensus
rules covered, 0 gaps` (GAS-1..GAS-7 covered).
`cargo xtask claims-check`: `97 public files scanned, 0 unauthorised claims`.

## NOT VERIFIED

- `cargo xtask coverage --verify-targets` and the workspace clippy ratchet:
  left to CI (AGENT-RULES).
- Pulley parity: `tier_differential_tests` is ignored on Windows; only CI
  on Linux runs it, now with failed-call gas, warm/cold compiled calls and
  storage reads in the corpus.
- Release-profile timing of whole blocks; slower validator hardware; ARM.
- Golden `gas_used` vectors were produced on this Windows machine; CI on the
  other platforms is what checks they agree.
