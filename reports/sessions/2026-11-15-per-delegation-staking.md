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
