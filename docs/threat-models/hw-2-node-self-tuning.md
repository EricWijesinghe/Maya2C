# Threat model: node hardware self-tuning (HW-2)

ADR: [ADR-137](../adr/ADR-137-hardware-intelligence.md) §HW-2.
Written 2026-11-12, revised the same day after the security-architect pass
(ADR-137 S-9, S-18, S-19). Tests: pending (plan step 3, `[post-genesis]`).

## Attacker

- A remote peer or RPC caller who wants to know which validator is weakest.
- Whoever controls the environment the node starts in (a container runtime,
  a hosting panel) and can make probe files say anything.
- A future contributor who reads a profile value inside a consensus rule.

## Assets

- **Determinism**: two honest nodes on different hardware reach the same
  decision and state root for every block (invariant 24).
- Liveness of the node (memory, file handles, threads).
- The operator's hardware details.

## Attacks

| # | Attack | Path |
|---|---|---|
| N-1 | **Profile leaks into validity.** A later change reads `verify_threads` or a cache size where a block is judged. | `TuningProfile` is constructed in the binary and passed only to the storage options and the prewarm call; a source-scan test lists the modules allowed to name it. |
| N-2 | **Parallel verification changes an outcome** (a bad signature slips in at some thread count). | Prewarm runs in the block builder's candidate selection, not in block application. It caches CPU `verify_at` successes only; every later check goes through the cache, where a hit is an earlier CPU success for the same bytes. Pinned separately for the builder (bad signatures at the first, middle and last position and at each chunk boundary) and for block application. |
| N-3 | **Hostile probe values**: memory `u64::MAX`, zero cores, a negative or huge cgroup limit, the "no limit" values (`max`; the very large number cgroup v1 uses). | Parse failure, an out-of-range value or a "no limit" value is `Unknown`; every derived knob is clamped to a compiled range; arithmetic is checked. |
| N-4 | **Container lie**: host reports 256 GiB, the cgroup allows 2 GiB; auto sizes the cache for the host and the kernel kills the node. | The smaller of host memory and the cgroup limit is used; caches stay under a compiled share of that. |
| N-5 | **Fingerprinting**: the status tells the network which validator has the least memory or a spinning disk. Loopback is not access control here: the public tunnel runs on the same host, so a tunnelled call arrives from loopback. | The method is absent unless the operator enables it; when enabled it refuses any request with a forwarding header and any non-loopback peer. The public status carries nothing hardware-derived. |
| N-6 | **Timing fingerprint**: tuned nodes answer faster. | Accepted. Hardware speed is observable with or without this feature. |
| N-7 | **Tuning a protocol knob**: someone adds BFT timeouts, mempool size or peer counts to the profile "for slow machines". | The ADR's deny list; the profile type has exactly the listed fields and a test asserts the field set. |
| N-8 | **Config drift**: `[tuning]` in a config read by an older binary refuses to start a validator after a rollback. | Rollout note: add the section only after upgrading; `mode = "off"` is the default so the section is optional. |
| N-9 | **Thread oversubscription** with the miner or the GPU verifier on the same host starves consensus threads. | On a host with more than two cores, verification threads are capped below the core count by the profile; BFT and network threads are not drawn from it. |
| N-10 | **CPU-feature branch**: the node adds its own AVX path because the probe says AVX-512, and it differs by one bit on one CPU model. | Features are reported only. The node adds no feature-gated arithmetic under this ADR. |

## Mitigations

As the table. `mode = "off"` reproduces today's behaviour exactly and is the
recommended default until a measured before/after exists (O-4).

## Replay tests (`crates/node/tests/tuning_profile_tests.rs`, `bins/maya2c-node` unit tests)

1. `every_tuning_profile_gives_the_same_decisions_and_state_root` (block application)
2. `prewarm_threads_do_not_change_what_the_builder_selects` (bad signatures at each chunk boundary)
3. `tuning_profile_is_named_only_by_the_allowed_modules`
4. `the_profile_has_exactly_the_listed_fields`
5. `a_hostile_probe_value_is_unknown_or_clamped`
6. `the_cgroup_limit_wins_over_host_memory`
7. `unknown_memory_or_storage_keeps_todays_defaults`
8. `an_explicit_storage_value_beats_auto`
9. `mode_off_builds_the_same_storage_options_as_before`
10. `the_tuning_method_is_absent_unless_enabled` and `the_tuning_status_is_refused_when_a_forwarded_header_arrives`
11. `the_public_status_carries_no_hardware_derived_field`
12. `verification_threads_stay_below_the_core_count_above_two_cores`
13. `a_cgroup_unlimited_sentinel_is_unknown_not_a_limit`
