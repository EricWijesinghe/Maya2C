# Threat model: Hub GPU auto-tuner (HW-1)

ADR: [ADR-137](../adr/ADR-137-hardware-intelligence.md) §HW-1.
Written 2026-11-12, revised the same day after the security-architect pass
(ADR-137 S-1 to S-8, S-17). Tests: pending (plan steps 1, 2 and 2b). Not consensus.

Two stages: **T1a guided** (the Hub suggests a limit, the user sets it, the
Hub measures; nothing is written and no rights are needed) and **T1b
automatic** (an elevated helper; gated). Rows marked *(T1b)* apply only to
the second.

## Attacker

- Malware or another user-level process on the miner's machine (can edit
  files in the Hub's data folder, can put programs on `PATH`).
- Script running in the Hub webview (a compromised page or dependency).
- A hostile or buggy driver tool output.
- The user's own optimism: settings that work for an hour and crash at night.

Out of scope: an attacker who is already administrator or root.

## Assets

- The GPU and the machine (heat, stability, lifetime).
- Administrator rights, which the helper holds for a moment.
- The Hub's standing with operating-system stores and antivirus products.
- The miner's income (a mis-tuned card earns less or produces stale shares).

## Attacks

| # | Attack | Path |
|---|---|---|
| T-1 | **Webview sets the watts.** A script calls the tuner command with its own value. | The command takes a device and on/off only; values are computed in the backend from the driver range. |
| T-2 | **Profile poisoning.** The stored profile is edited to an extreme. | A profile is a hint: clamped to the live driver range and the envelope, discarded on a driver-version change. |
| T-3 | **Journal tampering**: a hostile value to "restore", a deleted journal, a `trying` entry edited to done, a file from another user or for another card. | Restoring is always to the driver default, never to a value from a file. A missing, unreadable, unknown or foreign-owned journal counts as a trial in progress: tuning off. Entries are keyed by GPU UUID. |
| T-4 | **Tool hijack.** A fake `nvidia-smi` on `PATH`, or planted beside the Hub (on Windows the program's own folder is searched first), runs at user level today and elevated through the helper tomorrow. | Driver tools are resolved by absolute system path and never by search; before any elevated use the signer is checked. |
| T-5 | *(T1b)* **Helper as a privilege ladder.** Messages smuggle a path, a flag or a shell; another process talks to the resident helper. | The helper accepts only `(uuid, watts)`, authenticates the other end of its one channel, re-reads the range for every message, has a lifetime cap, and sets the driver default when it exits. It links nothing but the standard library. |
| T-6 | *(T1b)* **Helper replaced on disk** before elevation. The miner's pin is not enough: it stages a copy in a user-owned folder, and elevating that copy gives any user-level process root (S-1). | The helper is elevated only from a per-machine path that only an administrator can write, with the release signature (Windows) or root ownership, mode and a polkit action bound to its path (Linux), checked just before the prompt. Until those exist there is no helper. |
| T-7 | **Crash loop.** A setting that crashes the machine is reapplied on every start. | Write-ahead marker and journal; a `trying` marker at start is a bad mark and tuning off. Fails closed (T-3). Residual: user-level malware that rewrites marker and journal consistently can clear a bad mark; the envelope still bounds every value. |
| T-8 | **Slow cook.** Telemetry freezes at a cool value while the card heats, or the memory runs hot while the core reads cool. | Any missing reading aborts the trial; memory temperature is used where the driver reports it; the existing thermal stop stays independent of the tuner. T1 only lowers power, so it cannot add heat over the default; a card with no memory temperature is not eligible for fan or clock stages. |
| T-8a | **Abort makes it worse.** A thermal abort restores the previous, higher limit on a hot card. | An abort holds the lower limit until the card is under the ceiling minus the hysteresis. |
| T-9 | **Silent refusal.** The driver accepts the call and changes nothing, so the search "optimises" noise. | Every write is read back; no read-back, no trial. |
| T-10 | **Garbage in the efficiency figure** (power reads zero, hash rate spikes) picks a bad step. | A window with any missing, zero-power or non-finite sample is void; a step needs a full valid window. |
| T-11 | **Antivirus or store flag** from a kernel driver or undocumented interface. | None is used: documented vendor tools only, no driver, no service, no autostart. |
| T-12 | **Tuning leaks into other work**: the card stays capped for a game or for an inference job. | Original restored when mining on that device stops; HW-4 restores before a job. |
| T-13 | **Wrong hashes from an unstable card** look like hash rate. | The miner does not re-check GPU results on the CPU today and has no `hw_error` event; plan step 1 adds both. Then one event aborts the trial. |
| T-15 | **Wrong card.** Two tools number the cards differently and the limit lands on another GPU. | Cards are named by UUID in the profile, journal and helper message, resolved before each write. |
| T-16 | **Card cycling.** Repeated limit writes (the tuner, or HW-4's scheduler) wear the card. | A compiled cap on writes per hour per card. |
| T-17 | *(T2, design only)* **Pinned fan outlives a crash.** | Every exit path releases the fan to the driver's automatic control; T2 is unavailable where a set-then-release self-test fails. |
| T-14 | **Two Hub instances** fight over one card. | Single-owner lock file per device beside the journal; the second instance shows read-only state. |

## Mitigations

As the table. The envelope is compiled; the user can tighten and never widen
it. **Residual:** a card that fails only after hours at a setting that passed
its dwell window. The watchdog's crash limit catches the crash, and the step
is marked bad when the crash is attributed (miner exit within the watch
period after a profile is applied); it cannot be caught before it happens.

## Replay tests (`apps/hub/src-tauri/src/tuner.rs`, `gpu_control.rs`, helper tests)

1. `the_tuner_command_takes_no_watt_value_from_the_caller`
2. `a_stored_profile_outside_the_driver_range_is_clamped_or_discarded`
3. `an_unreadable_or_missing_journal_is_treated_as_a_trial_in_progress`
4. `driver_tools_are_resolved_by_absolute_path_never_by_search`
5. `a_planted_driver_tool_beside_the_hub_is_never_run`
6. `a_card_is_targeted_by_uuid_not_by_index`
7. `a_trial_left_in_trying_state_is_rolled_back_and_marked_bad_on_start`
8. `a_missing_reading_aborts_the_trial_and_is_never_a_guess`
9. `a_suggested_limit_that_does_not_read_back_is_not_measured`
10. `a_window_with_a_zero_power_or_non_finite_sample_is_void`
11. `no_step_ever_exceeds_the_default_limit`
12. `a_thermal_abort_never_raises_the_limit_while_hot`
13. `one_hw_error_event_aborts_the_trial`
14. `a_second_instance_cannot_tune_a_device_the_first_owns`
15. `the_search_stops_at_the_hash_rate_floor_and_keeps_the_previous_step`
16. `a_journal_written_for_one_gpu_never_restores_onto_another`
17. `a_profile_or_journal_owned_by_another_user_is_ignored`
18. `limit_writes_per_hour_are_capped_per_card`
19. By hand, on a named GPU, output in the PR: kill the Hub mid-trial, restart,
    confirm tuning is off and the step is marked bad. NOT VERIFIED until run.

T1b only (step 2b, gated):

20. `the_helper_refuses_anything_but_a_uuid_and_an_in_range_limit`
21. `the_elevated_helper_is_never_run_from_a_user_writable_path`
22. `a_helper_without_the_release_signature_is_never_elevated`
23. `a_helper_swapped_between_check_and_prompt_is_refused`
24. `the_helper_sets_the_default_and_quits_at_its_lifetime_cap`
25. `the_helper_sets_the_default_when_its_channel_closes`

T2 only (design): `a_pinned_fan_is_released_on_every_exit_path`.
