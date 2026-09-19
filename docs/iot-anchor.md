# IoT anchor: hardware-anchored sensor identity

**Status: RESEARCH.** `IOT_ACTIVATION_HEIGHT = u64::MAX`. The code ships and
is tested, but no network runs it.

A device gets its signing key from a PUF or from a seed sealed in a TPM. Its
owner enrolls it on chain, and it then signs telemetry batches. Consensus
records each batch as the device's latest, flags readings outside the declared
bounds, and marks the device terminal on a signed tamper event, on clone
evidence, or on revocation by its owner.

| Piece | Where |
|---|---|
| Wire types, signing, fuzzy extractor, TPM sealing, pure rules (`no_std`, heap-free) | `iot-anchor/` |
| Example Cortex-M33 firmware under QEMU (its own workspace) | `iot-firmware/` |
| Transaction kinds and apply path | `src/core/iot_payload.rs`, `src/state/iot.rs`, `src/state/iot_exec.rs` |
| RPC `iot_device` | `src/rpc/server.rs` |
| End-to-end tests | `tests/iot_anchor_tests.rs` |
| Fuzzing | `fuzz/fuzz_targets/iot_anchor_decode.rs` |

## What the brief asked for, and what this is instead

| Brief | Why not | Instead |
|---|---|---|
| "Post-quantum **lightweight** signatures" | No lightweight post-quantum signature exists. Invariant 4 compiles only ML-DSA-65. The chain's hybrid adds SLH-DSA, which is out of reach of a microcontroller. Stateful LMS/XMSS is small, but restoring a flash image reuses one-time keys. | **ML-DSA-65 alone**: 3,309-byte signature, 1,952-byte key. It is weaker than the hybrid, and stated as such. `fips204` is `no_std` and heap-free, and this crate builds for `thumbv8m.main-none-eabihf` and `riscv32imc-unknown-none-elf`. |
| A crate "supporting PUFs" | Software is not a PUF. | A **fuzzy extractor** (`puf.rs`) that turns a noisy response into a stable seed. Tests prove the error correction, not unclonability. |
| TPM 2.0 / secure element key generation | No TPM 2.0 or common secure element generates ML-DSA keys. Their attestation certificates are a vendor's classical signature, which makes the vendor a trusted party (the enclave collision, invariant 11). | Hardware **seals the seed** (`tpm.rs`, PCR-bound) and never signs. No vendor certificate is verified on chain. |
| Secure enrollment | Nothing proves genuine silicon without a vendor certificate. | The owner's transaction carries the device's **proof of possession**, which binds owner, device, class and bounds. |
| A real-time pipeline "in the DAG" | Consensus has no DAG. One signature per reading is 3.3 KB. | **Batches**: the device signs a Merkle root, a counter range and the min/max. Raw readings stay off chain. Freshness is block height (invariant 9). |
| Validating physical readings | A signature proves the key, not the physics. | Authenticity and counter order are enforced. Bounds are **flagged, never refused**, and never an error (invariant 7). |
| Tamper detection on chain | A chain senses nothing physical. | It records a **signed tamper event** and **clone evidence** (conflicting batches), and reports **silence**. Silence is not proof. |

## Device side

1. **Manufacture.** A TRNG draws a 256-bit secret. `HelperData::enroll(response,
   secret)` stores public helper data: a 15× repetition-code offset plus a check
   value.
2. **Boot.** `PufSource` reads the SRAM response, and `HelperData::reproduce`
   corrects up to 7 flipped bits per group. The recovered seed expands with
   `DeviceKey::from_seed`. A response past the correction budget is an error,
   never a different key.
3. **Enroll.** `DeviceKey::enroll(owner, class, bounds, rng)`.
4. **Measure.** `ReadingsAccumulator` commits to up to 86,400 contiguous
   readings using 64 hashes of memory. `sign_batch` signs the summary.
5. **Tamper.** On an enclosure, voltage, clock or light trigger, firmware calls
   `sign_tamper`, transmits the event, and zeroizes its seed.

Each message kind signs under its own ML-DSA context
(`maya2c-iot-{enroll,batch,tamper}-v1`), so a signature cannot move between
kinds.

The repetition code is chosen for auditability. It leaks through the helper
data if response bits are biased or correlated, so production SRAM needs
debiasing or a stronger code first.

### TPM sealing (Linux gateways)

`TpmSealed` (feature `tpm`) runs `tpm2-tools` against the TPM named by
`TPM2TOOLS_TCTI`. It seals the seed under a PCR policy, with a storage primary
re-derived on each use. After a PCR is extended, unsealing fails. The ignored
test `a_sealed_seed_unseals_and_a_changed_pcr_refuses_it` checks this against
`swtpm`. No C library is linked.

### Firmware under QEMU

`iot-firmware/` runs the whole device lifecycle on an emulated Cortex-M33
(`mps2-an505`): PUF seed, enrollment, a 60-reading batch, and a tamper event. It
reports the stack high-water mark, painted below the stack pointer. QEMU has no
TRNG and no PUF, so both are stand-ins, and the run demonstrates the code path,
not security. Cycle counts are not reported because QEMU does not model timing.

**Measured on 2026-09-19:** 258,532 bytes of stack for the whole lifecycle,
built with `opt-level = "s"` and LTO under QEMU 8.2.2. That rules out parts with
256 KiB of RAM or less. ESP32-C3 (400 KiB SRAM) and larger Cortex-M33 parts fit;
most Cortex-M0/M3 parts do not. The bulk is ML-DSA-65's stack-allocated state
plus the 5.3 KB enrollment kept on the stack. Signing time on real silicon is
still unmeasured.

## Chain side

| Transaction | Authorised by | Module (breaker) | Refused (error) | No-op |
|---|---|---|---|---|
| `EnrollDevice` | owner = sender, device proof | `Iot` | bad proof, device already enrolled | — |
| `SubmitTelemetry` | device signature (any gateway submits) | `Iot` | bad signature | unknown device, duplicate, stale, unlinked, terminal device |
| `ReportTamper` | device signature | none | bad signature, unknown device | terminal device |
| `ProveEquivocation` | two device signatures (anyone submits) | none | not conflicting, bad signature, unknown device | terminal device |
| `RevokeDevice` | owner = sender | none | not the owner, unknown device | terminal device |

Tamper reports, clone evidence and revocation are never gated by the circuit
breaker. A breaker that halted revocation while a key was being abused would
protect the attacker, which is the reasoning that leaves HTLC claims ungated.

### The batch rules (`rules.rs`, Kani-proved: 3 harnesses, 0 failures, Kani 0.67.0)

Each batch signs `previous`: the hash of the device's last batch, or all zeros
for its first. The recorded batches of a device therefore form one hash chain,
and a batch is judged against the head of that chain:

- **Terminal device.** It records nothing.
- **The head again.** A second relay: `Duplicate`, no change.
- **Extends the head** (names the head as its predecessor). It is `Recorded` if
  its counters move forward, and flagged `anomalous` if its min or max is out of
  bounds or moved more than `max_step`. If it reuses counters the head already
  covered, it is `Equivocation`.
- **Forks or overlaps the head.** A second successor of the head's own
  predecessor, or overlapping counters with different content, is
  `Equivocation`, and the device becomes `Compromised`.
- **Older and unrelated.** A late relay: `Stale`, no change.
- **Ahead, unlinked.** Its predecessor is not recorded yet, because it was
  relayed out of order: `Unlinked`, no change. Gateways retry it once its
  predecessor lands.

Why a hash chain rather than counters alone: with counters, a copied key could
post a batch far ahead of the genuine device's counter, so no range ever
overlapped, and every genuine batch would then read as stale. The genuine
device would be silently muted. With the chain, the clone has to extend the
recorded head to be recorded at all, and the genuine device's next batch
extends the same predecessor. That fork is detected in state the moment it
lands. If the clone races further ahead first, the owner's gateway, whose batch
was not recorded, files the two batches that share a predecessor as
`ProveEquivocation` evidence, which verifies without any state.

The proofs show:

- a terminal device records nothing;
- every recorded batch extends the head, or starts the chain, and moves the
  counter forward;
- equivocation requires different, genuinely conflicting content;
- a duplicate relay is never equivocation.

### State

`v:dev:<device id>` holds a `DeviceRecord` (2,139 bytes): owner, key, class,
bounds, status, enrolment height, and the latest batch. Updates overwrite the
record, so state grows by device count, not by reading count. The record is in
`RECORD_LAYERS` under `StateLayer::Iot`, and the undo journal restores it on a
reorg (invariants 8 and 25). An absent layer folds nothing, so no existing
state root changes.

## Limits, stated

- **ML-DSA-65 alone is not the hybrid.** A break of ML-DSA breaks device
  identity.
- **Firmware must persist its chain.** A device keeps its last batch hash
  across reboots. One that loses it and restarts at the chain start forks its
  own chain, and is marked `Compromised`, exactly as a clone would be.
- **Clone detection in state covers one step.** A clone that extends the head
  more than once before the genuine device's next batch lands is caught by
  filed evidence, not by state. That needs the owner's gateway to watch for its
  own unrecorded batches.
- **Signed does not mean true.** An analog-side attack produces honest
  signatures over false readings. The tests pin that such a batch is recorded.
- **Enrollment proves possession, not silicon.**
- **Microcontroller cost.** Stack is measured in QEMU. Signing time on a real
  M33 or ESP32-C3 has not been measured, and Cortex-M0/M3-class parts are not
  expected to fit.
- **Helper data leaks** under biased PUF bits (see above).
- **Gateway censorship.** A gateway may withhold batches, which shows up only as
  silence. Devices should use more than one gateway.
