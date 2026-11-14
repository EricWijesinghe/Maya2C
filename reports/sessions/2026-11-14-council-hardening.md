# 2026-11-14: council hardening, review fixes (PR #544)

PR #544's first commit (385a6858) plus the review-fix commit. Machine: the
owner's Windows 11 PC, builds through `cargo-slot.sh`. Real output below,
trimmed to the result lines.

## Red: the cooldown read from the breaker record

The pause cooldown temporarily put back to the PR's first version (read from
`g:guard:<module>`, filtered to `CouncilPause`), with the new unit test:

```text
$ cargo test -p custom-l1-node --lib state::council
running 3 tests
test state::council::tests::a_pending_rotation_takes_over_at_its_activation_and_not_before ... ok
test state::council::tests::the_record_round_trips ... ok
test state::council::tests::an_automatic_trip_does_not_reset_the_council_renewal_cooldown ... FAILED

failures:

---- state::council::tests::an_automatic_trip_does_not_reset_the_council_renewal_cooldown stdout ----

thread 'state::council::tests::an_automatic_trip_does_not_reset_the_council_renewal_cooldown' (47208) panicked at crates\node\src\state\council.rs:584:14:
called `Result::unwrap_err()` on an `Ok` value: ()
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    state::council::tests::an_automatic_trip_does_not_reset_the_council_renewal_cooldown

test result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 544 filtered out; finished in 0.06s
```

## Green

```text
$ cargo test -p custom-l1-node --lib council
running 9 tests
test state::upgrades::tests::a_far_council_sunset_does_not_block_a_later_governance_upgrade ... ok
test state::council::tests::a_pending_rotation_takes_over_at_its_activation_and_not_before ... ok
test core::council_payload::tests::a_sunset_flag_byte_other_than_0_or_1_is_refused ... ok
test state::upgrades::tests::a_council_sunset_after_a_pending_upgrade_does_not_veto_a_later_upgrade ... ok
test core::council_payload::tests::a_rotation_missing_an_acceptance_does_not_decode_and_accepting_is_not_approving ... ok
test core::council_payload::tests::a_rotation_naming_more_members_than_approvals_can_reach_is_refused ... ok
test core::council_payload::tests::actions_round_trip_and_the_nonce_is_signed ... ok
test state::council::tests::the_record_round_trips ... ok
test state::council::tests::an_automatic_trip_does_not_reset_the_council_renewal_cooldown ... ok

test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 538 filtered out; finished in 0.05s

$ cargo test -p custom-l1-node --test council_pause_tests
running 17 tests
test a_genesis_council_of_more_than_32_members_is_refused ... ok
test the_committed_council_read_after_activation_is_the_new_one ... ok
test a_council_sunset_has_no_emergency_window_so_no_path_exits_a_frozen_vault_early ... ok
test approvals_signed_for_another_chain_are_refused ... ok
test a_rotation_with_too_few_approvals_is_refused ... ok
test a_rotation_past_the_schedule_horizon_is_refused ... ok
test a_council_sunset_is_scheduled_only_at_the_council_lead_and_never_as_a_version ... ok
test the_council_can_neither_pause_nor_resume_governance ... ok
test a_later_rotation_replaces_a_pending_one_and_clears_the_lead_again ... ok
test a_rotation_naming_a_key_nobody_signed_for_is_refused ... ok
test a_pause_expires_without_anyone_acting ... ok
test a_council_sunset_cannot_follow_another_within_the_cooldown ... ok
test a_rotation_must_clear_the_lead_and_name_a_sound_set ... ok
test a_council_pause_that_expires_unresumed_cools_down_from_its_end ... ok
test a_rotation_takes_effect_at_its_height_and_not_before ... ok
test a_council_pause_cannot_be_renewed_within_the_cooldown ... ok
test a_council_quorum_pauses_one_module_transfers_continue_and_the_pause_ends ... ok

test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.09s

$ cargo test -p genesis-ceremony council
running 12 tests
test council::tests::module_names_map_to_their_tags ... ok
test council::tests::approvals_for_another_chain_do_not_combine ... ok
test council_rotate::tests::a_set_the_chain_would_refuse_is_refused_before_anyone_signs ... ok
test council::tests::a_repeat_is_refused_even_when_it_would_reach_the_majority ... ok
test council::tests::two_of_three_combine_into_an_action_that_round_trips ... ok
test council::tests::no_approvals_or_an_index_past_the_council_is_refused ... ok
test council::tests::malformed_approval_files_are_errors_not_panics ... ok
test council::tests::a_council_directory_with_a_sealed_secret_a_repeat_or_junk_is_refused ... ok
test council::tests::half_is_not_a_majority ... ok
test council::tests::a_message_with_trailing_bytes_is_refused ... ok
test council::tests::one_approval_a_repeat_a_forgery_or_a_different_action_is_refused ... ok
test council_rotate::tests::every_new_member_must_accept_once_for_this_rotation ... ok
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 17 filtered out; finished in 0.03s

$ cargo test -p custom-l1-node --lib describe
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 536 filtered out; finished in 0.00s

$ cargo xtask spec-coverage --strict
CNL-1    03-state.md             0    0  covered (external vectors)
CNL-2    03-state.md             0    0  covered (external vectors)
CNL-3    03-state.md             0    0  covered (external vectors)
CNL-4    03-state.md             0    0  covered (external vectors)
66 rules, 164 vectors; 63 consensus rules covered, 0 gaps: 

$ cargo xtask claims-check
claims-check: 97 public files scanned, 0 unauthorised claims
```

`cargo clippy -p genesis-ceremony --all-targets` (which also lints the node
library as a path dependency): two `vec_box` warnings in the new
`bins/genesis-ceremony/src/council_rotate.rs` (the node's acceptance type),
allowed with a comment afterwards; that allow was not re-linted. No warning
pointed at `state/council.rs`, `core/council_payload.rs`, `core/describe.rs`
or `genesis.rs`.

## NOT VERIFIED

- `cargo xtask coverage --verify-targets` and workspace clippy: left to CI
  (agent rules, 2026-11-12).
- Node integration test targets other than `council_pause_tests` were not run
  locally.
- `features.toml` stays `working` until CI is green on PR #544.
