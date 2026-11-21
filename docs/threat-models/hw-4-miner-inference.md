# Threat model: inference jobs on miners (HW-4)

ADR: [ADR-137](../adr/ADR-137-hardware-intelligence.md) §HW-4; consensus rules
in ADR-129 (PR #470) and [ADR-079](../adr/ADR-079-verifiable-compute.md).
Written 2026-11-12, revised the same day after the security-architect pass
(ADR-137 S-13 to S-16). Tests: pending (plan steps 6 and 7, `[post-genesis]`).

This file covers what HW-4 adds: the device manifest, the Hub's worker mode
and the scheduler. Escrow, bonds, agreement and slashing are ADR-129's and
its own review applies.

## Attacker

- A job requester who wants the worker's machine to run something other than
  an allow-listed runtime, or to lose its bond.
- A rival worker who wants this worker slashed or idle.
- A worker who over-states its hardware to win jobs.
- Malware on the worker's machine.

## Assets

- The worker's bond and key file.
- The worker's machine (arbitrary code execution is the worst case).
- The meaning of "device class" to a requester reading it.
- Mining income lost to bad scheduling.

## Attacks

| # | Attack | Path |
|---|---|---|
| I-1 | **Code in a job.** The spec names a container or binary the requester controls. | The Hub starts only runtimes and models on the allow-list signed with the release, checked by hash; a job is data. |
| I-2 | **Model swap on disk** between download and run. | Content hash checked at load against the allow-list entry, with the held-handle pattern from `miner_guard.rs`. |
| I-3 | **Bond drain.** Many jobs sized to overrun, so the worker misses deadlines or produces mismatches. | User's bond cap; a job is taken only if its declared bound fits the measured time for that runtime on this card with margin; one mismatch pauses worker mode. |
| I-4 | **Instability slash.** The card is tuned, by the Hub or by another tool, when a job starts. | A job is refused unless the power limit and clocks read back at the **driver default**, not at whatever was there before tuning. |
| I-5 | **Class inflation.** A manifest claims a larger card; a worker runs its own calibration jobs and the label reads as proof. | The label is "declared", or "calibrated for runtime X at height H (self-run)", never "verified" or "attested". Evidence is per runtime and expires. Calibration never raises the bond cap. |
| I-6 | **Calibration replay.** One worker's old calibration is presented by another. | The evidence is the worker's own ADR-129 claim on chain for a calibration job, keyed by the worker's address. |
| I-7 | **Rented or proxied hardware.** | Accepted and stated: evidence is about results, not ownership. The UI never says "verified hardware". |
| I-8 | **Preemption griefing.** Cheap jobs timed to keep the card out of mining. | The user's rule sets a price floor and a share of time for jobs; below it the scheduler stays in `Mining`. |
| I-9 | **Thermal stacking.** A job starts while the card is in a thermal hold from mining. | No assignment during a hold; the ceiling applies in every state. |
| I-10 | **Private input exposure.** Job inputs sit on the worker's disk. | Inputs are deleted when the claim is sent; the page states that a worker can read what it computes. Confidential inputs are ADR-079's later rungs, not this one. |
| I-11 | **Key reuse.** The worker key is the mining payout key. | A separate worker key file under the same permission checks (`key_acl.rs`); the bond cap bounds the loss. |
| I-12 | **"AI decides."** A model chooses which jobs to take or how much to bond. | The scheduler is rules the user set. A suggested schedule changes nothing until the user accepts it. |
| I-13 | **Runtime escape by data.** A crafted input exploits the runtime's parser and reads the worker key or the network. | The runtime is a child process with no key-file access, no network and a scratch folder as its only writable path; an input naming a path outside scratch is refused; the claim is signed in another process. |
| I-14 | **Card cycling.** Jobs timed so the card flips between mining and inference. | A minimum dwell per scheduler state, and HW-1's cap on limit writes per hour. |

## Mitigations

As the table. **Open:** the content and liability question of running
strangers' prompts (ADR-137 O-10) is legal, and no test settles it.

## Replay tests (`apps/hub/src-tauri/src/scheduler.rs`, `compute.rs`)

1. `a_job_outside_the_signed_allow_list_is_never_started`
2. `a_model_whose_hash_differs_from_the_allow_list_is_refused_at_load`
3. `bond_exposure_never_passes_the_users_cap`
4. `a_job_whose_bound_does_not_fit_the_measured_time_is_not_taken`
5. `a_job_is_refused_unless_power_and_clocks_read_back_at_default` and `no_job_is_taken_during_a_thermal_hold`
6. `a_device_class_is_labelled_declared_or_calibrated_self_run_never_verified`
7. `calibration_evidence_is_per_worker_per_runtime_and_expires`
8. `an_assigned_job_is_never_abandoned_for_mining`
9. `below_the_price_floor_the_scheduler_stays_in_mining`
10. `one_mismatch_pauses_worker_mode_and_says_why`
11. `job_inputs_are_removed_after_the_claim_is_sent`
12. `the_fake_worker_says_it_is_a_fake_in_its_output`
13. `calibration_never_raises_the_bond_cap`
14. `an_inference_runtime_cannot_read_the_worker_key`
15. `a_job_input_naming_a_path_outside_scratch_is_refused`
16. `the_scheduler_never_cycles_the_card_faster_than_its_dwell`
