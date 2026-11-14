# 2026-11-15 — Per-delegation staking records (ADR-062b)

Branch `refreeze/per-delegation-staking`, from `origin/master` at 2090300c.
Eric 2026-11-14 MCQ: "per-delegation records". Machine: the shared Windows
development PC, test profile (node at opt-level 0), one build slot. Timings
below are single runs on that profile, not benchmarks. Release-profile
figures: MISSING.

## Red, on master

The dust test written against master's API (100,000 delegations of
`min_delegation` to one validator, then one new delegation to another),
run on master's `crates/node` and `crates/staking`:

```
test state::staking_exec::red_062b::a_crowd_of_minimum_delegations_does_not_block_a_new_one ... FAILED
a new delegation beside 100,000 dust ones: Err(Network("staking refused: the record would be 9601031 bytes, above 8388608"))
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 543 filtered out; finished in 0.51s
```

## Green, on this branch

`cargo test -p custom-l1-node --lib -- state::staking genesis::tests invariant_guard rpc::validators --nocapture --test-threads=1`:

```
delegate beside 10 delegations: 3 records, 1.8775ms; beside 100000: 3 records, 71.5µs
slash of a validator with 100000 delegations: 100002 records, 904.7425ms
1000 delegations: plain block 1 records 143.1µs; boundary 5 records 219.2µs; next block's payouts 1002 records 5.1826ms
100000 delegations: plain block 1 records 140.3µs; boundary 5 records 216.2µs; next block's payouts 1026 records 6.6273ms
seed 1: accepted {"bond_more": 28, "delegate": 186, "register": 2, "slash": 16, "unbond": 27, "undelegate": 54}, payouts opened 118
seed 2: accepted {"bond_more": 26, "delegate": 162, "register": 2, "slash": 10, "unbond": 22, "undelegate": 67}, payouts opened 95
seed 3: accepted {"bond_more": 29, "delegate": 214, "register": 2, "slash": 13, "unbond": 29, "undelegate": 58}, payouts opened 121
seed 4: accepted {"bond_more": 9, "delegate": 51, "register": 2, "slash": 12, "unbond": 6, "undelegate": 25}, payouts opened 30
seed 5: accepted {"bond_more": 31, "delegate": 148, "register": 2, "slash": 13, "unbond": 24, "undelegate": 73}, payouts opened 107
seed 6: accepted {"bond_more": 15, "delegate": 117, "register": 2, "slash": 13, "unbond": 17, "undelegate": 46}, payouts opened 45
test result: ok. 52 passed; 0 failed; 0 ignored; 0 measured; 493 filtered out; finished in 7.40s
```

After adding the ceiling test, `-- state::staking`: `test result: ok. 16 passed; 0 failed`.

"Records" is staking records read, counted in `staking_store` (cfg(test)).
The read counts are the claim; the times are what one run showed.

- A delegation reads 3 staking records whether the chain holds 10 or
  100,000 other delegations.
- A plain block reads 1 (the meta record).
- A boundary reads 5 (meta plus four validators).
- The pass after a boundary reads its budget, 1,024 delegations, plus its
  payout keys.

The randomized test compares the store to `maya_staking::Staking` ended with
`end_epoch` (every delegator credited at the boundary, the single-record
rules). It runs 6 seeds × 900 steps and checks two things:

- After every step: validators, delegations, unbonding entries, committee,
  retired ids, pending stake, and each validator's aggregates.
- Every 150 steps: every balance and the held total, once every payout has
  drained.

**Mutation check.** Paying pending stake (`let earning = d.amount;` in
`settle`) made it fail with
`Storage("staking state inconsistent: a payout owes more than it holds")`.
Reverted.

Integration suites, `cargo test -p custom-l1-node --test …`:

| suite | result |
|---|---|
| bft_staking_tests | 8 passed |
| bft_follower_tests | 13 passed |
| cert_rpc_tests | 7 passed |
| conformance | 6 passed |
| exploit_replays | 11 passed |
| genesis_tests | 23 passed |
| supply_property_tests | 1 passed |
| `maya-light-client` finality_tests | 21 passed |
| `maya-staking` (unit, offender_unbond, scenario) | 6 + 6 + 13 passed |

Gates:

- `cargo xtask spec-coverage --strict`:
  `70 rules, 164 vectors; 67 consensus rules covered, 0 gaps`.
- `cargo xtask claims-check`: `97 public files scanned, 0 unauthorised claims`.
- `cargo clippy -p maya-staking -p custom-l1-node --lib`: no warning in a line
  this change wrote. The one warning in a touched file,
  `conservation.rs:251` `unused self`, is on `fold_balance`, unchanged from
  master.

## Not run

- `cargo xtask coverage --verify-targets` and the workspace clippy ratchet:
  left to CI (AGENT-RULES).
- Kani (`cargo kani -p maya-staking`): needs WSL. The harnesses call
  `register`, `delegate`, `slash_double_sign` and `held`, which this change
  did not touch; `end_epoch` was refactored and has no harness.
- `cargo xtask bench-tps` before and after: not run; release-profile cost:
  MISSING.

## Review round 1 (same day)

Fixes for the first review of PR #555. Same machine and profile; single runs.

### Red, before the fixes

The new tests run against this PR's first commit's `staking_store/{mod,commit}.rs`
(0c69a836), with only the two new constant names added so they compile:

```
test state::staking_vectors_tests::staking_records_and_layer_root_match_the_spec_vectors ... FAILED
test state::scan_bounds_tests::a_block_of_staking_transactions_costs_each_one_the_same ... ok
test state::staking_store_tests::a_replayed_equivocation_report_reads_one_validator_and_writes_nothing ... FAILED
test state::staking_store_tests::a_crowd_at_one_validators_ceiling_leaves_every_other_validator_open ... FAILED
test state::staking_store_tests::the_payout_pass_walks_each_delegation_once_across_open_payouts ... ok
test state::staking_store_tests::a_downtime_boundary_over_a_dust_crowd_rewrites_no_delegation ... FAILED
downtime boundary over 16384 dust delegations: 16389 records read, 16384 delegation records written, 70.1163ms
assertion `left == right` failed: an unchanged delegation is not rewritten
  left: 16384
 right: 0
test result: FAILED. 2 passed; 4 failed; 0 ignored; 0 measured; 554 filtered out; finished in 0.24s
```

The vectors test was red because the vectors did not exist yet. The
64-transaction test passes on 0c69a836 too: it pins that #522's 16-per-block
count is not needed; it is not a behaviour change. The payout-pass test also
passed on the old code, which showed that the cursor sharing it was written
for never changes the walk (a validator's lower payout is always finished
first). The sharing was removed; the test stays, renamed, as a churn test with
a stated re-read bound.

`end_epoch` golden digest, recorded by running
`crates/staking/tests/golden_end_epoch.rs` against 2090300c's `crates/staking/src/`:

```
assertion `left == right` failed: digest 0x54d521a662c0fdd3 of
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

### Green

`cargo test -p custom-l1-node --lib`:
`test result: ok. 555 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 6.75s`

`cargo test -p custom-l1-node --lib -- --nocapture --test-threads=1 staking_store_tests`:

```
delegate beside 10 delegations: 3 records, 251.1µs; beside 100000: 3 records, 74.3µs
downtime boundary over 16384 dust delegations: 16389 records read, 0 delegation records written, 75.482ms
slash of a validator with 16384 delegations: 16386 records, 82.886ms
1000 delegations: plain block 1 records 110.1µs; boundary 9 records 251.1µs; next block's payouts 1016 records 4.3887ms
100000 delegations: plain block 1 records 112.7µs; boundary 9 records 190.1µs; next block's payouts 1026 records 2.8352ms
pass over two payouts of 3000 delegations: 4108 records read
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 550 filtered out; finished in 4.27s
```

The crowd is now spread over eight validators, so a boundary reads 9 records.
The removal and re-registration test, added after this run, passed:
`test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 560 filtered out`.

Block cost against delegation records, including the state root
(`cargo test -p custom-l1-node --lib -- --ignored --nocapture --test-threads=1 staking_cost_against_delegation_records`):

```
staking delegation_records= 10000 over 1 validators: settle_staking=20µs state_root=9.0413ms top_up=44.4µs
staking delegation_records=100000 over 7 validators: settle_staking=60.7µs state_root=102.0147ms top_up=40.2µs
staking delegation_records=131072 over 8 validators: settle_staking=23.1µs state_root=142.7631ms top_up=32.1µs
staking slash of 16384 delegations: median=69.2891ms
```

#522's generic root figures on this branch, in an earlier run of the same session:

```
state-root accounts= 100000: median=141.9155ms
state-root records = 100000 (200 B each): median=182.4854ms
```

The state root over 131,072 delegation records (142.8 ms) is below what
master's 87,000-delegation record cost in `settle_staking` alone (250–289 ms,
#522). The first draft's ceiling of 1,843,200 records was not measured; scaled
from the figure above it would be about 14 times the 131,072 figure (an
extrapolation, NOT VERIFIED). Release profile: MISSING.

`cargo test -p custom-l1-node --test bft_staking_tests --test bft_follower_tests --test cert_rpc_tests --test conformance --test exploit_replays --test genesis_tests --test supply_property_tests`:
8, 13, 7, 6, 11, 23 and 1 passed.

`cargo test -p maya-staking`: unit 6, golden_end_epoch 1, offender_unbond 6, scenario 13 passed.

Gates:

- `cargo xtask spec-coverage --strict`: `72 rules, 183 vectors; 69 consensus rules covered, 0 gaps`.
- `cargo xtask claims-check`: `97 public files scanned, 0 unauthorised claims`.
- `cargo clippy -p maya-staking --all-targets`: one warning, `crates/amount/src/lib.rs:223`, not this change.
- `cargo clippy -p custom-l1-node --lib --tests`: no warning in a file this round
  touched, except existing ones in `invariant_guard/` (`anomaly.rs`,
  `conservation.rs:251`, `mod.rs:211`).

### Not run in round 1

- Release-profile timings, `cargo xtask bench-tps`: MISSING.
- `cargo xtask coverage --verify-targets`, workspace clippy ratchet: left to CI.
- Kani: not run (WSL only).
