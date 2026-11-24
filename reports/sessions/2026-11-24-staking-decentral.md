# 2026-11-24 — Staking and decentralization guarantees (re-freeze)

Branch `refreeze/staking-decentral-1124`, stacked on `test/s033-floor-node-halt` (#524).
Eric 2026-11-24: the recommended option for each item, each documented as a guarantee.

## What was built

| Item | Guarantee | Code | Tests |
|---|---|---|---|
| S-033 (option A) | Punishment alone never shrinks the committee below four seats | #524 (unchanged); spec STK-11 | `at_mainnet_bonds_downtime_never_shrinks_the_committee_below_four_seats`, `at_mainnet_bonds_the_committee_floor_ends_the_halt_when_it_restarts` (not ignored) |
| CON-07 | No exit breaks the floor: a whole unbond waits, a tombstone keeps its seat (at most one) until a replacement bonds | `crates/staking/src/{ops,epoch,types}.rs`, record v4 in `crates/node/src/state/staking.rs` | `crates/staking/tests/committee_exits.rs` (6), `a_waiting_exit_round_trips_and_only_its_presence_changes_the_version` |
| CON-05 | Every double-sign is slashable: attestation pairs at the proposal rate | `crates/node/src/state/staking_exec.rs` | `an_attestation_double_sign_is_slashed_at_the_proposal_rate` |
| CON-06 | No operator holds more than 20% of the vote; excess stake still earns | `crates/node/src/state/weight_cap.rs` | 5 unit tests incl. a 2,000-committee sweep |
| C4, C5 | The node refuses a genesis over `max_validators`, or a value-bearing one under seven seats | `crates/node/src/genesis/staking_rules.rs` | `the_loader_refuses_a_committee_outside_its_bounds`, ceremony `mainnet_needs_seven_validators_a_testnet_does_not` |
| Genesis rules | Every penalty is in the genesis id; a mainnet must write them | `staking_rules.rs`, ceremony `LAUNCH_STAKING_RULES`, `xtask genesis-check` | `the_staking_rules_are_part_of_the_genesis_id_and_are_the_rules_run`, `unwritten_staking_rules_fail`, `the_launch_rules_are_the_values_the_node_has_always_run` |
| G6 | A vote weighs locked stake only | `crates/node/src/state/governance_exec.rs` | `claimed_work_neither_votes_nor_counts_toward_the_quorum` |
| G4 | Every governable key binds something | `crates/governance/src/params.rs`, `crates/node/src/state/shielded.rs` | `a_key_the_node_does_not_read_is_refused_and_the_rest_are_offered`; Kani harnesses updated |

## Red first (mutations, restored afterwards)

- CON-07 disabled (no deferral, no hold): `cargo test -p maya-staking --test committee_exits` → `test result: FAILED. 2 passed; 4 failed`.
- CON-06 disabled (`level.filter(|_| false)`): `cargo test -p custom-l1-node --lib -- weight_cap` → `FAILED. 4 passed; 3 failed` (whale, many keys, sweep).
- CON-05 disabled (attestation branch returns an error): `--test bft_staking_tests -- attestation` → `FAILED. 0 passed; 1 failed`.
- G6 disabled (work credit back in the electorate): `--test governance_tests -- claimed_work` → `FAILED ... left: Rejected, right: Executed`.
- C4/C5 and the written rules: the API did not exist before (compile-level red).

## Green (local, through cargo-slot)

- `cargo test -p maya-staking -p maya-governance`: 30, 8, 2, 6, 6, 6, 14 passed, 0 failed.
- `cargo test -p custom-l1-node --lib --test bft_staking_tests --test governance_tests --test bft_follower_tests --test genesis_tests`: lib `548 passed; 0 failed; 1 ignored`; bft_follower `15 passed`; bft_staking `11 passed`; genesis `23 passed`; governance `25 passed`.
- After moving the `max_validators` check into `check_committee`: genesis 23, bft_staking 11, bft_follower 15 passed.
- `cargo test -p genesis-ceremony`: `33 passed; 0 failed`.
- `cargo test -p xtask -- genesis_check proving`: `21 passed; 0 failed` (with origin/master's `xtask/src/devlog.rs` in the working tree only: the base branch's copy does not compile, `chrono` was removed; not committed).
- `cargo xtask spec-coverage --strict`: `71 rules, 164 vectors; 68 consensus rules covered, 0 gaps`; STK-11, STK-13..STK-20 `covered (external vectors)`.
- `cargo xtask claims-check`: `100 public files scanned, 0 unauthorised claims`.
- `cargo clippy -p custom-l1-node --lib`: no warning in a changed file. `cargo clippy -p maya-staking -p maya-governance --all-targets`: two `doc_markdown` warnings in `crates/governance/src/limits.rs`, untouched here.

## NOT VERIFIED

- Clippy on the node's test targets, genesis-ceremony and xtask; workspace pedantic ratchet: left to CI.
- `cargo xtask coverage --verify-targets`: left to CI (agent rule).
- Kani harnesses in `crates/governance/src/proofs.rs`: edited, not run (Kani runs in WSL/CI).
- `ShieldedMaxPerBlock` below 16: no test (a joinsplit needs a STARK per transaction).
- `cargo xtask ceremony-rehearsal` and `cargo xtask proving` end to end.
- No attacknet run.
