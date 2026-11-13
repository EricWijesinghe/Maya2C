# Threat model: GPU signature verification (HW-3)

ADR: [ADR-137](../adr/ADR-137-hardware-intelligence.md) §HW-3.
Written 2026-11-12, revised the same day after the security-architect pass
(ADR-137 S-10 to S-12): stage G2 (mempool reordering) was removed, so the
approved stages are G0 (harness) and G1 (shadow beside the builder's
prewarm). Tests: pending (plan steps 4 and 5; step 5 is `[post-genesis]`).

## Attacker

- Anyone who can submit transactions or blocks and can craft signatures that
  sit on encoding and bound edges.
- The GPU stack itself as a *faulty* party: a shader-compiler bug, a driver
  update, a bit flip in memory without error correction, a card running
  under-volted by HW-1.
- A flooder sending invalid signatures to consume verification time.

Out of scope: a *malicious* driver. It runs in the kernel and already owns
the node. The design must still not turn a faulty GPU into a wrong verdict.

## Assets

- **Validity**: no transaction or block is accepted that the CPU verifier
  refuses, and none is refused that it accepts (invariants 24, 29, 30, 31).
- The verified-signature cache (successes only, CPU-written).
- Node liveness under a flood.

## Attacks

| # | Attack | Path |
|---|---|---|
| G-1 | **False accept.** The GPU says valid for a forged signature. | The GPU answer decides nothing: it never writes the cache, never admits, never imports. Fake always-yes backend test. |
| G-2 | **False reject as censorship.** The GPU says invalid for a valid transaction and the node drops or delays it. | The GPU answer is compared and discarded; admission and import never consult it. Fake always-no backend test. |
| G-3 | **Reordering by GPU answer** (the draft's G2): a delayed valid transaction is evicted by fee in a flood, and any false-accept class moves an attacker's garbage to the front. | Removed. Admission verifies synchronously; there is no queue and none is added. |
| G-4 | **Edge encodings** where the two implementations differ: malformed hints, a response coefficient exactly at the bound, non-canonical lengths. | G0 mutation suite on every field; ACVP verification vectors including the failing ones. |
| G-5 | **Second door around `verify_at`.** GPU code is handed a frame whose validity depends on height or policy. | Only hybrid (v5/v6) frames go to the GPU; suite-tagged (v7) and multisig (v8) frames never do (invariant 31). |
| G-6 | **Half a hybrid.** One half verifies on the GPU and the result is read as the hybrid's. | The GPU reports each half separately; nothing consumes a combined verdict. |
| G-7 | **Parameter drift.** A generic shader is pointed at another parameter set. | ML-DSA-65 and SLH-DSA-SHA2-128s dimensions are constants; no parameter argument. |
| G-8 | **Resource exhaustion.** Huge batches or a device loss wedge the verifier thread; one long dispatch trips the operating system's GPU watchdog and resets a card the miner shares. | Work is cut into bounded dispatches with a bounded wait; on timeout or device loss the GPU is marked unavailable and the CPU path continues unassisted. Off by default, and documented as not for a card that is mining or being tuned. |
| G-9 | **Buffer reuse.** A stale result from the previous batch is read as this batch's. | Each batch carries a nonce written into the output header by the shader and checked by the host; results are matched by index and wire hash. |
| G-10 | **Mismatch log as a leak or a flood.** | Logs carry wire hashes and the two answers only; rate-limited; no key bytes. |
| G-11 | **Dependency creep.** `crypto-pq` or `crates/node` gains `wgpu` by default. | `hal/wgpu-verify` depends on `maya-crypto-pq`, never the reverse; the node's `gpu-verify` feature is off by default and absent from the production feature set until decided. |
| G-12 | **"It agreed a million times."** Pressure to promote the GPU to authoritative on shadow statistics. | ADR-137 finding 2 and O-6: a count of agreements does not bound the next answer. Promotion needs its own ADR. |
| G-13 | **A "hash-bound split" built from the ADR's sketch as if it were safe.** A wrong GPU value refuses a valid signature (a chain split on a consensus path), and a GPU-supplied SLH-DSA digest or index is a false-accept path. | Not approved. A later ADR must give every GPU-path refusal a full CPU verification and keep the digest and indices on the CPU. |

## Mitigations

As the table. **Residual at the approved stages:** the GPU can waste time (a
slower node) and, on a shared card, disturb the miner. It cannot change a
verdict, an order or a delay.

## Replay tests

G0, `hal/wgpu-verify/tests/differential.rs`:

1. `acvp_sigver_vectors_agree_with_the_cpu_for_ml_dsa_65`
2. `acvp_sigver_vectors_agree_with_the_cpu_for_slh_dsa_sha2_128s`
3. `every_single_field_mutation_gets_the_cpu_answer`
4. `a_response_at_the_norm_bound_and_one_past_it_agree_with_the_cpu`
5. `malformed_hints_and_wrong_lengths_agree_with_the_cpu`
6. `a_batch_result_carries_its_own_nonce_and_a_stale_buffer_is_refused`
7. `the_shader_has_no_parameter_set_argument`
8. `a_lost_device_returns_unavailable_not_a_verdict`

G1, `crates/node/tests/gpu_verify_tests.rs` (fake backends, no GPU needed):

9. `gpu_verdict_never_reaches_the_verified_cache`
10. `a_block_with_a_gpu_accepted_bad_signature_is_refused`
11. `a_gpu_refused_valid_transaction_is_still_admitted_and_mined`
12. `mempool_admission_does_not_consult_the_gpu`
13. `one_mismatch_latches_the_gpu_off_for_the_process`
14. `the_gpu_reports_each_half_and_never_a_hybrid_verdict`
15. `suite_tagged_and_multisig_frames_never_reach_the_gpu`
16. `block_import_does_not_consult_the_gpu`
17. `the_node_builds_and_passes_without_the_gpu_verify_feature`
18. `a_gpu_batch_is_cut_into_bounded_dispatches`
19. `the_gpu_module_holds_no_reference_to_the_verified_cache` (source scan)
