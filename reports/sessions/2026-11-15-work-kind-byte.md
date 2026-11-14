# 2026-11-15: work-kind byte and work-claim guard (re-freeze)

Branch `refreeze/work-kind-byte`. Eric 2026-11-14 MCQ ("all format fixes").
ADR-044 amendment 2026-11-15; spec MINE-11.

## Red (tests written first)

At this stage the `WorkKind` field existed, the wire format was unchanged (no
kind byte), and neither guard was in place.

```
     Running tests\governance_tests.rs
test a_work_claim_is_refused_at_mempool_admission ... FAILED
thread 'a_work_claim_is_refused_at_mempool_admission' (56044) panicked at crates\node\tests\governance_tests.rs:416:5:
Ok(true)
test result: FAILED. 25 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.89s

test a_share_and_a_claim_encode_kind_zero_first ... FAILED
test every_work_kind_byte_but_zero_is_refused_at_decode ... FAILED
thread 'a_share_and_a_claim_encode_kind_zero_first' (37732) panicked at crates\node\tests\payload_tests.rs:994:5:
  left: [7, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0]
 right: [0, 7, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0]
thread 'every_work_kind_byte_but_zero_is_refused_at_decode' (38320) panicked at crates\node\tests\payload_tests.rs:977:10:
payload in frame
test result: FAILED. 29 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.26s

test work_claim_cannot_be_scheduled_until_its_adr_is_accepted ... FAILED
thread 'work_claim_cannot_be_scheduled_until_its_adr_is_accepted' (44180) panicked at crates\node\tests\upgrade_schedule_tests.rs:334:53:
genesis schedule refused: UpgradeSchedule { upgrades: [ProtocolUpgrade { version: 2, activation_height: 2693443, features: FeatureSet(16) }] }
test result: FAILED. 7 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.02s
```

## Green

`cargo test -p custom-l1-node --no-fail-fast --test payload_tests --test upgrade_schedule_tests --test governance_tests --test mining_rewards_tests --test conformance`:

```
     Running tests\conformance.rs
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.49s
     Running tests\governance_tests.rs
test a_work_claim_is_refused_at_mempool_admission ... ok
test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.59s
     Running tests\mining_rewards_tests.rs
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 8.02s
     Running tests\payload_tests.rs
test a_share_and_a_claim_encode_kind_zero_first ... ok
test every_work_kind_byte_but_zero_is_refused_at_decode ... ok
test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s
     Running tests\upgrade_schedule_tests.rs
test work_claim_cannot_be_scheduled_until_its_adr_is_accepted ... ok
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.11s
```

Other checks:

- `cargo run -p maya-spec-ref --bin spec-vectors`: the only file that changed was `spec/tests/mining.json`, which gained 6 MINE-11 cases.
- `cargo test -p maya-spec-ref`: `test result: ok. 8 passed; 0 failed`.
- `cargo xtask spec-coverage --strict`: `MINE-11  09-mining.md  2  4  covered`; `63 rules, 170 vectors; 60 consensus rules covered, 0 gaps:`; exit 0.
- `node spec/verifier-ts/verify.ts`: `{"state_transitions":20,"encoding":9,"fees":20,"headers":3,"mining":63,"keys":4} cases, 0 mismatches`.
- `cargo check -p maya2c-gpu-miner --all-targets`: `Finished`.
- `cargo clippy -p custom-l1-node --lib --tests -- -D clippy::all`: `Finished`, no errors. The pedantic warnings reported in the touched test files are all on lines this change did not write.

## NOT VERIFIED

- `cargo test -p custom-l1-node --lib` (the unit tests in `src/`): not run, because the test-mode lib build is the heaviest node target. The clippy run with `--lib --tests` type-checked it. Left to CI.
- `cargo xtask coverage --verify-targets` and the workspace pedantic ratchet: left to CI, per AGENT-RULES.
- The remaining node test targets, and every crate other than `custom-l1-node`, `maya-spec-ref` and `maya2c-gpu-miner`: left to CI.

## Review fixes (2026-11-15, second pass)

Reviewers found the `UpgradeSchedule::new` refusal of `WORK_CLAIM` forks at
the vote, the pattern #506 removes for zkML. The branch now merges #506
(`fix/upgrade-admit-unknown-bits` at df67b6fb), takes its side of the two
conflicts, and expresses the guard as
`IMPLEMENTED = KNOWN & !ZKML & !(if WORK_CLAIM_ADR_ACCEPTED {0} else {WORK_CLAIM})`.
Genesis refuses bit 4 at boot; a governance entry is admitted and halts this
binary at its activation. #506's `a_known_bit_entry_still_does_not_halt` now
proposes `FeatureSet::IMPLEMENTED`. Both executors destructure the kind
irrefutably (`let WorkKind::DagPow = ...`).

Red, genesis half (guard not yet in `IMPLEMENTED`):

```
test work_claim_halts_this_binary_at_activation_until_its_adr_is_accepted ... FAILED
panicked at crates\node\tests\upgrade_schedule_tests.rs:351:10:
a genesis naming work claim credit is refused at boot: UpgradeSchedule { upgrades: [ProtocolUpgrade { version: 2, activation_height: 2693443, features: FeatureSet(16) }] }
```

Red, governance half (test reordered, `WORK_CLAIM_ADR_ACCEPTED` temporarily `true`):

```
panicked at crates\node\tests\upgrade_schedule_tests.rs:346:10:
this binary halts at the activation: ()
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 11 filtered out
```

Green (`cargo test -p custom-l1-node --lib --test upgrade_schedule_tests --test payload_tests --test governance_tests --test mining_rewards_tests --test unknown_bit_halt_tests --test conformance --test genesis_tests`):

```
unittests src\lib.rs      test result: ok. 542 passed; 0 failed; 1 ignored
conformance.rs            test result: ok. 6 passed; 0 failed
genesis_tests.rs          test result: ok. 23 passed; 0 failed
governance_tests.rs       test result: ok. 26 passed; 0 failed
mining_rewards_tests.rs   test result: ok. 13 passed; 0 failed
payload_tests.rs          test result: ok. 32 passed; 0 failed
unknown_bit_halt_tests.rs test result: ok. 1 passed; 0 failed
upgrade_schedule_tests.rs test result: ok. 12 passed; 0 failed
```

`cargo xtask spec-coverage --strict`: `65 rules, 170 vectors; 62 consensus rules covered, 0 gaps`.

NOT VERIFIED locally: `bft_node_tests` (changed by #506, not by this pass),
clippy, `coverage --verify-targets` (left to CI). Wrong-length MINE-11 vectors
were not added (optional in review).
