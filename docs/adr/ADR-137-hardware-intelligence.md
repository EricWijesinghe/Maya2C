# ADR-137: Hardware intelligence: a GPU tuner in the Hub, a self-tuning node, GPU signature checks that stay advisory, and inference jobs scheduled around mining

**Status:** Proposed, 2026-11-12. Eric chose all four items (HW-1 to HW-4) on 2026-11-12, so their scope is decided; the open questions O-1 to O-6 and stage G3 are still Eric's, so the status stays Proposed until he closes them. HW-1 is Hub-only and its guided stage can ship before genesis; its automatic stage is gated (see HW-1). HW-2 and the node half of HW-3 change the frozen node binary, so they are `[post-genesis]` draft PRs. HW-4 is `[post-genesis]` and waits for ADR-129.
**Date:** 2026-11-12
**Related:** [ADR-079](ADR-079-verifiable-compute.md) (verifiable compute), ADR-129 (job market and inference receipts, P-47, bit 35 `COMPUTE_JOBS`, PR #470), [ADR-073](ADR-073-hub-multi-rig-pool-profitability.md) and [ADR-083](ADR-083-hub-wallet-and-chat-pages.md) (Hub), [ADR-044](ADR-044-mining-rewards-lane.md), [ADR-007](ADR-007-signature-suites.md), [ADR-010](ADR-010-entropy.md), [ADR-002](ADR-002-feature-tiers.md), [ADR-004](ADR-004-reality-ledger.md); invariants 2, 3, 4, 9, 11, 20, 24, 29, 30, 31, 33
**Threat models:** [hw-1](../threat-models/hw-1-gpu-auto-tuner.md), [hw-2](../threat-models/hw-2-node-self-tuning.md), [hw-3](../threat-models/hw-3-gpu-signature-verification.md), [hw-4](../threat-models/hw-4-miner-inference.md). **Prior art:** [gpu-auto-tuning](../prior-art/gpu-auto-tuning.md), [node-hardware-self-tuning](../prior-art/node-hardware-self-tuning.md), [gpu-signature-verification](../prior-art/gpu-signature-verification.md); HW-4 uses `verifiable-inference.md` from PR #470.

## Context

"Hardware intelligence" here means software that reads what the machine is
and adjusts itself inside fixed limits. None of it is a model. HW-1 is a
bounded search, HW-2 is a lookup table, HW-3 is a second implementation
checked against the first, and HW-4 is a scheduler. The word "learns" in the
brief is kept only for HW-1, where it means "stores the best setting found
for this GPU".

What exists today, checked in the tree on 2026-11-12:

- **Hub telemetry is read-only and NVIDIA-only.** `apps/hub/src-tauri/src/gpu.rs`
  shells out to `nvidia-smi --query-gpu=...` and fills `GpuStat` (temperature,
  power draw, power limit, utilisation, fan, memory, clock). A field the driver
  does not report is `None`. The Hub writes no GPU setting anywhere.
- **The Hub already has a supervisor.** `watchdog.rs` is a pure state machine
  (time is passed in): crash back-off, a crash limit (5 in 10 minutes), a
  60-second stall check, and a thermal stop at the user's ceiling with 5 °C
  hysteresis. `miner.rs` clamps the ceiling to 50–95 °C and `MinerConfig`
  carries it per device. `miner_guard.rs` hash-pins the miner binary.
- **The miner reports events, not hardware errors.** `bins/maya2c-gpu-miner`
  emits `work`, `hashrate`, `share`, `idle`, `error` as JSON lines. There is no
  event for "the GPU returned a wrong hash".
- **The block builder already verifies signatures in parallel, safely.**
  `prewarm_verification` (`crates/node/src/state/db.rs:1169`) is called from
  the builder's candidate selection (`db.rs:1124`), not from block
  application. It fans the candidates over `available_parallelism()` threads
  with a compiled `MIN_PER_THREAD = 32`, into `VerifiedCache`
  (`state/verified.rs`). The cache stores **CPU successes only**, keyed by
  the wire hash and chain tag. Every later check (the sequential builder
  pass, mempool admission at `network/mempool.rs:352` and `:395`, block
  application) goes through `VerifiedCache::verify`: a hit returns at once,
  a miss runs `verify_at`. So a hit is always an earlier CPU `verify_at`
  success for the same bytes.
- **Mempool admission verifies synchronously** and a full pool evicts by fee
  (`network/mempool.rs`). There is no verification queue to reorder.
- **RocksDB sizes are configuration with fixed defaults.** `StorageConfig`
  (`crates/node/src/config.rs:111`) defaults `block_cache_mib = 512`; nothing
  reads the machine's memory.
- **GPU crates are miners.** `hal/cuda-miner` and `hal/wgpu-miner` each port
  one function and keep a CPU reference as the oracle. `wgpu-miner` records
  why it avoided 64-bit hashes: WGSL has no 64-bit integer type.
- **The hybrid signature is 11,165 bytes** (`crates/node/src/crypto/hybrid.rs:128`):
  ML-DSA-65 plus SLH-DSA-SHA2-128s. `reports/12-performance.md` measured
  hybrid verification as 93 % of block application before the cache existed.
- `hal/entropy` is REAL and verified; `hal/ebpf-net` and `hal/energy` are
  RESEARCH. No manifest in the tree depends on `maya-entropy` except its own.

### Findings that contradict the brief (Standing Order 9)

1. **"Only HW-4 is consensus-critical" is not quite the shape.** As designed
   here, **nothing in ADR-137 adds a consensus rule.** HW-4's consensus surface
   (records under `cj:`, `TxKind::ClaimCompute`, bit 35) belongs to ADR-129 and
   is not repeated. HW-4 itself adds off-chain parts only. The item that
   *could* become consensus-critical is HW-3, at the stage where a GPU answer
   is trusted without a CPU check. This ADR does not take that step.
2. **"Unless a differential test proves byte equality" cannot be met as
   written.** A differential test compares two implementations on a finite set
   of inputs. It is evidence, not a proof, and it says nothing about a GPU
   that computes correctly today and wrongly after a driver update, a bit flip
   in non-ECC memory, or an undervolt applied by HW-1 on the same card. So the
   rule here is stricter than the brief: the CPU stays authoritative at every
   stage this ADR approves.
3. **With the CPU authoritative, a GPU cannot speed up valid signatures.**
   Every valid signature still costs one CPU verification. The stages approved
   here buy measurement, not throughput. The one
   route to real throughput with a sound argument is sketched under HW-3 as a
   candidate for a later ADR.
4. **"Hardware attestation of the device class" has no trust-free form.** A
   vendor attestation is a trusted party's signature (the open conflict in
   ADR-079; CLAUDE.md lists TEE attestation as colliding with recorded
   invariants). HW-4 therefore uses a *declared* device class plus calibration
   evidence, and never the word "attested".
5. **An automatic tuner needs an elevated helper, and the Hub cannot ship
   one safely today.** Setting a power limit needs administrator or root.
   The security pass (S-1 to S-3) showed that the Hub's present install and
   pinning model does not protect an elevated binary, and the release
   signing key does not exist yet. So HW-1 ships first as a *guided* tuner
   that writes nothing, and the automatic one is gated on named conditions.
6. **`docs/threat-models/` does not exist on master.** PR #477 creates it with
   a `README.md`. This PR adds `hw-*.md` in the same format and leaves the
   index to whichever lands second.

## Decision

Four items, built in order **HW-1 → HW-2 → HW-3 → HW-4**. Each has a kill
switch that restores today's behaviour exactly.

### HW-1 Hub GPU auto-tuner (REAL, Hub only, pre-genesis allowed)

**What it does.** With the user's opt-in, per GPU, while that GPU is mining,
the tuner walks the card's power limit down step by step, measures hashes
per joule at each step, keeps the best step that holds the user's hash-rate
floor, and stores it as that GPU's profile. In the guided stage the user
applies each step in the vendor's own tool and the Hub only measures; in the
automatic stage the Hub applies it through a helper.

**Levers, staged.** Each stage needs its own opt-in.

| Stage | Lever | Bound | Status in this ADR |
|---|---|---|---|
| T0 | none (efficiency read-out: hash rate ÷ `power_w`) | read-only | build |
| T1a | power limit, **guided**: the Hub shows the next value to try, the user sets it, the Hub sees it by read-back and measures | the Hub suggests only values between the driver's reported minimum and its default; it writes nothing and needs no rights | build |
| T1b | power limit, **automatic**, downward only, through the helper | same range; never above default | **gated**: needs the helper conditions below |
| T2 | fan speed, **upward only** | never below what the driver's automatic control chose; every exit path releases the fan to the driver's automatic control; available only if a set-then-release self-test shows automatic control resumes | design only, after T1b |
| T3 | clock offsets | driver-reported range | **not approved**; open question O-1 |

**The search.** Deterministic, no model: start at the driver default; step
down by a fixed fraction of the allowed range; wait a settle time; measure
mean hash rate and mean power over a dwell window; stop at the first step
whose efficiency is worse than the previous one, or whose hash rate is under
the floor; keep the previous step. Step size, settle and dwell are compiled
constants chosen in the build PR from measurement, not taken from this text.

**Safety envelope (compiled, the user can tighten it and never widen it).**

- A write is refused unless the driver reported the constraint range in this
  session; every write is read back; a value that does not read back aborts.
- A trial aborts on: temperature (GPU, and memory where the driver reports
  it) at or above the user's ceiling minus a margin; miner exit; stall
  (`STALL_MS`); a rejected-share rate above the baseline window; a `hw_error`
  event (new, see plan); any missing reading. A missing reading is never a
  guess.
- **An abort never raises the limit on a hot card.** The previous value is a
  *higher* limit than the trial. On a thermal, stall or exit abort the tuner
  holds the current lower limit and returns to the previous one only once
  the card is under the ceiling minus `THERMAL_HYSTERESIS_C`.
- **A card is named by its UUID**, never by an index, in the profile, the
  journal and the helper's argument, and is resolved again before each write.
  Enumeration order can differ between tools.
- **Crash rollback is write-ahead and fails closed.** Before a trial value is
  applied, the Hub writes a `trying` marker and the entry `{uuid, default,
  previous, trial}` and syncs both. On start, a marker without a matching
  completed entry, an unreadable journal, or a journal with unknown fields
  all count as "a trial was in progress": the step is recorded as bad for
  that GPU and driver version and tuning for that GPU stays off until the
  user turns it on again. A journal or profile owned by another user is
  ignored. Restoring is to the **driver default**, never to a value read
  from a file.
- A stored profile is a hint. On load it is clamped against the live driver
  range and the envelope; a profile for another driver version or another
  UUID is discarded.
- At most a compiled number of limit writes per hour per card, so neither
  the tuner nor HW-4's scheduler can cycle a card.
- Tuning runs only while the Hub window shows it: a banner with the GPU,
  the step, the watts and a stop button. No background service, no autostart.

**Vendor paths.**

| Vendor / OS | Path | In first build |
|---|---|---|
| NVIDIA, Windows and Linux | read: `nvidia-smi` by absolute system path, never by search (on Windows the process search order tries the program's own folder first, so a file planted beside the Hub would run today). Write (T1b): `nvidia-smi -pl <watts>` from the helper | read yes; write gated |
| AMD, Windows | ADLX `IADLXManualPowerTuning` (range, then `SetPowerLimit`) | no, PLANNED; the UI says "not wired" as it does today |
| AMD / Intel, Linux | hwmon `power1_cap` within `power1_cap_min`/`_max` | no, PLANNED |

**Privilege.** Setting a power limit needs administrator or root. The Hub
never runs elevated. T1a needs no rights at all. T1b needs a helper, and
the helper is the most dangerous part of this ADR, so it ships only when
**all** of these hold (O-2):

- it is installed per machine in a location only an administrator can write
  (never the Hub's per-user folder, never a staged copy in a user-owned
  temp folder, which is where `miner_guard.rs` stages the miner today);
- on Windows it carries the release Authenticode signature and the Hub
  checks the signer before asking for elevation; on Linux it is root-owned,
  its owner and mode are checked just before launch, and the polkit action
  names its exact path. The release signing key does not exist yet;
- one prompt starts one helper for one tuning session with a compiled
  lifetime cap; it accepts only `(uuid, watts)` messages over one channel
  whose other end it authenticates, re-reads the driver range itself for
  every message, refuses anything outside it, resolves the driver tool by
  absolute system path and checks its signer, and on exit, on a closed
  channel or at the cap sets the driver default and quits;
- it accepts no path, flag or command, and links the standard library only.

**Platform compliance.** T1a changes nothing on the machine. No kernel driver, no MSR or SMBus access, no
undocumented vendor interface, no firmware write. Windows Defender began
flagging the WinRing0 driver used by several fan and monitoring tools in
March 2025; this design has no driver to flag. Opt-in per device, visible
while active, off by default.

**Not claimed.** Any efficiency gain is **MISSING** until measured on a named
GPU with commit, driver, OS and command. The tuner optimises the Maya2C
hashimoto workload only.

### HW-2 Node hardware self-tuning (REAL, node binary, `[post-genesis]`)

**What it does.** At start the node builds a `HardwareProbe` (logical cores,
CPU features AVX2 / AVX-512F / NEON, memory, the storage class of the data
directory, whether a GPU adapter exists) and derives a `TuningProfile` from
it with a pure integer function. The profile sets:

| Knob | Today | Under a profile |
|---|---|---|
| builder prewarm verification threads | `available_parallelism()` | same source, capped by the profile |
| builder prewarm `MIN_PER_THREAD` | compiled 32 | profile value within a compiled range |
| `block_cache_mib`, `write_buffer_mib`, `max_open_files` | config defaults | derived from memory and storage class, under a compiled share of memory |
| RocksDB background jobs | library default | derived from cores |
| GPU verification (HW-3) | absent | `off` unless HW-3 enables a stage |

**The rule that makes it safe.** A profile may change *how fast* a result is
produced and never *which* result. It may set only the knobs in the table.
It may not touch: any validity check, mempool admission limits, gossip or
peer limits, BFT timeouts, pruning depth, or anything a peer can observe as
protocol behaviour. CPU features are *reported*; the node adds no SIMD code
of its own because of them (the cryptographic crates already select their
own paths at run time).

**Why it cannot change a result.** Prewarm only adds CPU `verify_at`
successes to a cache; whether a later check hits the cache or runs
`verify_at` itself, the answer is the same function of the same bytes.
Cache and buffer sizes do not change what RocksDB returns. That argument is
pinned by tests, not left as prose, and separately for the two paths,
because prewarm runs in the builder and not in block application: (1) the
builder, given the same candidates with bad signatures at chosen positions,
selects the same transactions under the smallest, default, largest and
deliberately absurd profiles; (2) the same blocks applied under each
profile give identical accept/refuse decisions and identical state roots.

**Override and exposure.** `[tuning] mode = "off" | "auto" | "manual"` in the
node config. `off` is today's behaviour byte for byte. An explicit value in
`[storage]` or `[tuning]` always beats `auto`. One log line at start states
the profile and every value chosen. An RPC method `get_node_tuning` reports
the same, and it is **absent unless the operator enables it**. When enabled
it answers only a loopback peer whose request carries no forwarding header
(`CF-Connecting-IP`, `X-Forwarded-For`): the public tunnel runs on the same
host, so a tunnelled call also arrives from loopback, and loopback alone is
not access control. A public hardware description tells an attacker which
validator to exhaust first.

**Unknown is an answer.** A probe that cannot read something reports
`Unknown`, and `Unknown` maps to today's default for that knob. Memory inside
a container is the smaller of the host figure and the cgroup limit; the
"no limit" values (`max`, and the very large number cgroup v1 uses) are
`Unknown`, not a limit.

**Not claimed.** Any speed-up is **MISSING** until `cargo xtask bench-tps`
runs before and after on named hardware.

### HW-3 GPU signature verification (RESEARCH, advisory)

**Does it parallelise? Honestly:**

- **Across signatures, yes.** Each verification is independent, so a batch
  maps onto GPU threads. Within one signature the work is mostly sequential.
- **ML-DSA-65.** Verification is SHAKE-128 expansion of a 6×5 matrix,
  number-theoretic transforms modulo q = 8,380,417, and SHAKE-256 hashing.
  Keccak uses 64-bit lanes but only XOR, AND, NOT and rotation, which split
  into 32-bit halves without carries. The transform needs 32×32→64-bit
  products, which WGSL does not have and must be built from 16-bit limbs.
- **SLH-DSA-SHA2-128s.** Verification is SHA-256 only (32-bit, native in
  WGSL): 14 FORS trees of height 12, then 7 hypertree layers of 35 WOTS+
  chains of at most 15 steps each plus a height-9 authentication path. The
  chains are sequential inside a signature and parallel across chains and
  signatures.
- **What works against it.** Each signature is about 11 KB to upload; a batch
  pays a dispatch and read-back latency; the CPU path is already multi-core
  and cached. A block holds fewer than 470 hybrid signatures by bytes alone
  (5 MiB ÷ 11,165 B; invariant 33), so what one block can use is small,
  though a builder's candidate set can be larger. Published GPU
  results for these schemes are CUDA on large batches and mostly for signing
  (see the prior-art file). **No number is carried over: the speed-up for
  Maya2C is MISSING, and "the GPU is slower" is a valid G0 result.**

**Staging.** The CPU is authoritative at every approved stage.

| Stage | What the GPU answer may do | Where | Approved here |
|---|---|---|---|
| G0 | nothing: a crate and a differential harness | new `hal/wgpu-verify`, not linked by the node | yes |
| G1 | be compared with the CPU answer and counted (shadow mode), beside the builder's prewarm only | node, cargo feature `gpu-verify` off by default, config opt-in | yes, `[post-genesis]`, only if G0 passes its exit test |
| G2 | change the order in which pending transactions are CPU-verified | mempool | **removed from this ADR** (S-11): admission verifies synchronously and a full pool evicts by fee, so there is no queue to reorder, and inventing one lets a delayed valid transaction be evicted in a flood |
| G3 | let a GPU answer stand without a full CPU verification | consensus-critical | **no.** Needs its own ADR and Eric's decision (O-6) |

Rules that hold at G1:

- **The GPU never writes `VerifiedCache`** and holds no reference to it. Only
  a CPU `verify_at` success is cached. The GPU never causes a transaction or
  block to be dropped, refused, delayed or accepted.
- **One mismatch latches the GPU off** for the life of the process, logs both
  answers and the wire hash (never key material), and raises a status flag.
- Mempool admission and block import do not consult the GPU.
- **Only hybrid (v5/v6) frames are sent to the GPU**: their verification
  ignores height and policy. Suite-tagged (v7) and multisig (v8) frames
  never leave `verify_at` (invariant 31): the GPU is not a second door.
- Work is cut into dispatches short enough to stay well inside the
  operating system's GPU watchdog, with a bounded wait; a lost device means
  "unavailable", never a verdict.
- **Off by default, and documented as not for a card that is mining or
  being tuned.** The node cannot see the Hub's tuner, so this is an operator
  rule; the mismatch latch is the backstop.

**G0 exit test** (all must hold, output pasted in the PR): zero mismatches
against `maya-crypto-pq` on the ACVP signature-verification vectors already
in the tree, on generated valid signatures, and on single-field mutations of
each (challenge, response, hints, key halves, lengths, the response norm at
its bound and one past it); the same on a software adapter in CI and on at
least two vendors' drivers by hand; measured throughput against the cached
multi-core CPU path at batch sizes 32 to 469.

**G3 is an open question, not a design.** One idea is recorded so that a
later ADR starts from its problems. Instead of trusting a GPU verdict, the
GPU would return only intermediate values that the final hash comparison
binds (for ML-DSA the recomputed `w1`, which the CPU hashes and compares
with the challenge; for SLH-DSA upper tree nodes, which the CPU hashes to
the root). What that idea must still solve before anyone builds it:

- **A wrong GPU value makes a valid signature fail.** On a consensus path a
  false refusal is a chain split, not a safe failure. So every GPU-path
  refusal would need a full CPU verification before it counts, and only
  GPU-path acceptances would save work.
- **The CPU must compute everything the hash does not bind**: decoding, the
  response norm, hint weight, and for SLH-DSA the message digest and the
  FORS and tree indices derived from it. A GPU-supplied digest or index is a
  false-accept path the hash argument does not cover.
- It replaces the audited CPU verification with a split one, so it needs
  its own known-answer vectors (invariant 29) and its own review.

### HW-4 Inference jobs on miners (`[post-genesis]`, off-chain additions to ADR-129)

ADR-079 decided the shape and ADR-129 the records, the claim and the receipt.
HW-4 adds four things and no consensus rule, feature bit, state prefix or
transaction kind:

1. **Declared device class and calibration evidence.** The worker signs a
   `DeviceManifest` (vendor, adapter name, memory, driver version, the
   `runtime_id`s it can run). It is a *declaration*. What counts as evidence
   is a completed calibration job: a published deterministic job for a
   `runtime_id` whose commitment matched under ADR-129's agreement rule.
   Evidence says "this worker produced the right bytes for this runtime",
   not "this worker owns this card"; a worker that rents a card passes too,
   and that is acceptable because the chain pays for results. A worker can
   also create its own calibration jobs, so the label is exactly "calibrated
   for runtime X at height H (self-run)" and never "verified" or "attested".
   Calibration never raises the user's bond cap. Timing is not evidence.
2. **Hub integration.** A Compute page: opt-in worker mode supervising
   ADR-129's `maya2c-worker` under the same hash pin and key-file checks as
   the miner; jobs, bonds at risk and a bond cap the user sets; earnings read
   from chain balance growth as the mining page does.
3. **Scheduling around mining.** One GPU has one tenant. States: `Mining →
   Draining → Inference → Cooling → Mining`. A job is taken only if its
   deadline, memory bound and the thermal state fit and the user's standing
   rule allows it. An assigned job is never abandoned for mining (the bond is
   at stake). Inference runs at **driver defaults**: a job is refused unless
   the power limit and the clocks read back at the driver default (not at
   whatever value was there before tuning, which may be another tool's
   setting), because a wrong commitment from an unstable card is a slashed
   bond. Each state has a minimum dwell, and HW-1's writes-per-hour cap
   applies, so cheap jobs cannot cycle the card.
4. **Process boundary.** A runtime parses data from strangers. It runs in a
   child process with no access to the worker key file, no network, and a
   scratch folder as its only writable path; a job input that names a path
   outside the scratch folder is refused. Signing the claim happens in a
   different process.

**Rules.** Models are never on consensus. The Hub runs only runtimes and
models named in an allow-list signed with the Hub release; a job supplies
data, never code. The scheduler follows the user's rule; where the Hub
suggests a schedule it is a suggestion, and no bond is posted beyond the
user's cap. Device-class evidence is SIM until ADR-129's worker exists: the
build lane uses a fake worker that says so in its output.

## Trust boundaries

| Boundary | Trusted side | Untrusted input | Check |
|---|---|---|---|
| Hub webview → Tauri backend (HW-1, HW-4) | backend | command arguments | the webview names a device and on/off; it never supplies watts, paths or binaries |
| Hub → elevated helper (HW-1, T1b) | helper | UUID, watts | helper authenticates the caller, re-reads the driver range, refuses outside it; absolute signed tool path; no shell; lifetime cap |
| OS → helper binary (HW-1, T1b) | admin-only install path, release signature | the file on disk | never elevated from a user-writable path or without the signature |
| Hub ← driver tools (HW-1) | Hub | `nvidia-smi` output | existing parser: short, garbled, NaN or infinite is "no reading" |
| Hub ← profile and journal on disk (HW-1) | envelope | file content | clamped to live range; unreadable, unknown or foreign-owned means "trial in progress": tuning off, driver default |
| Node ← OS probes (HW-2) | compiled ranges | `/proc`, `/sys`, cgroup files | parse failure is `Unknown`; every derived value clamped |
| Node → RPC status (HW-2) | operator | remote callers, the local tunnel | method absent by default; loopback and no forwarding header when enabled |
| CPU verifier ← GPU answer (HW-3) | CPU | GPU output buffer | compared, never obeyed; mismatch latches off |
| Hub ← job specs and peers (HW-4) | allow-list | job fields | runtime and model must be allow-listed; data only |
| Hub worker ← inference runtime (HW-4) | Hub | the runtime process and the data it parsed | child process without key, network or paths outside scratch |
| Chain ← worker (HW-4) | ADR-129 rules | `ClaimCompute` | ADR-129; unchanged |

## Invariant analysis

| Invariant | Touched by | Finding |
|---|---|---|
| 2 (`crypto-pq` instantiates `slh-dsa`) | HW-3 | The letter holds: `hal/wgpu-verify` does not instantiate the `slh-dsa` crate and `crypto-pq` gains no GPU dependency. **But the shader is a second SLH-DSA verifier outside `crypto-pq`**, which the invariant's wording did not foresee. It is acceptable only because its answer decides nothing; a reviewer should confirm that reading before step 4 merges. |
| 3 (`cuda` default-off) | HW-3 | wgpu only; no CUDA path. The `gpu-verify` feature is default-off for the same reason: GPU-less runners must build. |
| 4 (`fips204` compiles only ML-DSA-65) | HW-3 | the shader has ML-DSA-65's dimensions as constants; no parameter argument exists to select another set. |
| 9, 20, 24 (no clock, no float, state root reproduced) | HW-2, HW-3 | nothing derived from hardware reaches the apply path; pinned by the cross-profile state-root test and a source scan that keeps `TuningProfile` out of apply-path modules. HW-1 and HW-4 use floats and clocks in the Hub, which is not consensus. |
| 11 (no unrecorded trusted party) | HW-4 | no vendor attestation; device class is declared. TEE attestation stays flagged, not designed. |
| 29 (registry suites only, each with known answers) | HW-3 | the GPU code implements two registry parameter sets and must pass their ACVP verification vectors. With G2 removed it is in no signature *path*: at G1 its answer is compared and discarded. |
| 30 (hybrid `0x30` byte for byte) | HW-3 | the GPU checks each half separately and reports each; it never reports a hybrid verdict from one half. |
| 31 (`verify_at` is the only door) | HW-3 | v7 and v8 frames never reach the GPU; only v5/v6 hybrid frames do. Tests `suite_tagged_and_multisig_frames_never_reach_the_gpu`, `gpu_verdict_never_reaches_the_verified_cache`. |
| 33 (block byte cap) | HW-3 | bounds what one block can hold, not the builder's candidate set or the mempool; cited, not changed. |

**Consensus-critical surface added by this ADR: none.** If O-6 is ever
answered "yes", that ADR adds one.

## Failure modes

| Item | Failure | What happens |
|---|---|---|
| HW-1 | machine hard-crashes mid-trial | marker is `trying` at next start: step marked bad, tuning off. T1a: the Hub shows the value it last suggested and the driver default. T1b: see next row |
| HW-1 | Hub killed while a limit is applied (T1b) | the helper sees the closed channel, sets the driver default and quits. If the helper died too, the lower limit stays (the safe direction) and the next Hub start says so and offers a restore, which needs a new prompt; the Hub cannot restore silently |
| HW-1 | journal or marker missing, unreadable or edited | treated as a trial in progress: tuning off for that GPU |
| HW-1 | driver refuses or silently ignores a write | read-back differs: abort, tuning unavailable for that GPU, reason shown |
| HW-1 | helper prompt declined, or helper conditions not met | tuner stays guided (T1a) and says so |
| HW-2 | probe wrong (container, VM, exotic disk) | `Unknown` or clamped; worst case is today's defaults or a compiled bound |
| HW-2 | profile exhausts memory | compiled share-of-memory cap; `mode = "off"` restores defaults |
| HW-3 | no adapter, device lost, GPU watchdog reset, driver update | CPU path alone; GPU marked unavailable. A reset on a card shared with the miner also stops the miner, which is why the default is off |
| HW-3 | GPU and CPU disagree | CPU answer used; GPU latched off; counted and logged |
| HW-4 | job overruns its window | never preempted; mining resumes after; the scheduler's estimate is corrected from the observed time |
| HW-4 | wrong commitment | ADR-129 slashes the bond; Hub pauses worker mode and shows why |
| HW-4 | card does not read back at driver default | job refused; in guided mode the Hub asks the user to reset the limit |

## Red-team list (each is a test the build lane writes first)

| # | Attempt | Pinned by |
|---|---|---|
| R-1 | Webview (or injected script) asks the backend for an out-of-range power limit | `the_tuner_command_takes_no_watt_value_from_the_caller` |
| R-2 | Edited profile file names 1 W or 10,000 W | `a_stored_profile_outside_the_driver_range_is_clamped_or_discarded` |
| R-3 | Crash during a trial, then restart | `a_trial_left_in_trying_state_is_rolled_back_and_marked_bad_on_start` |
| R-4 | Telemetry goes missing mid-trial | `a_missing_reading_aborts_the_trial_and_is_never_a_guess` |
| R-5 | A fake `nvidia-smi` on `PATH` or planted beside the Hub | `driver_tools_are_resolved_by_absolute_path_never_by_search`, `a_planted_driver_tool_beside_the_hub_is_never_run` |
| R-6 | Helper called directly with a path, a flag or an out-of-range value; helper run from a user-writable path, unsigned, or swapped before the prompt | `the_helper_refuses_anything_but_a_uuid_and_an_in_range_limit`, `the_elevated_helper_is_never_run_from_a_user_writable_path`, `a_helper_without_the_release_signature_is_never_elevated`, `a_helper_swapped_between_check_and_prompt_is_refused`, `the_helper_sets_the_default_and_quits_at_its_lifetime_cap` |
| R-7 | Tuner raises power above default, raises the limit on a hot card, or pins a fan | `no_step_ever_exceeds_the_default_limit`, `a_thermal_abort_never_raises_the_limit_while_hot`, `a_pinned_fan_is_released_on_every_exit_path` |
| R-7a | Journal deleted, edited to "done", or written for another card or by another user | `an_unreadable_or_missing_journal_is_treated_as_a_trial_in_progress`, `a_journal_written_for_one_gpu_never_restores_onto_another`, `a_profile_or_journal_owned_by_another_user_is_ignored` |
| R-7b | Two tools number the cards differently | `a_card_is_targeted_by_uuid_not_by_index` |
| R-8 | A profile changes what the builder selects or a block's fate | `prewarm_threads_do_not_change_what_the_builder_selects`, `every_tuning_profile_gives_the_same_decisions_and_state_root` |
| R-9 | Probe files return garbage, zero, or `u64::MAX` | `a_hostile_probe_value_is_unknown_or_clamped` |
| R-10 | Remote caller reads the hardware profile, directly or through the tunnel on the same host | `the_tuning_method_is_absent_unless_enabled`, `the_tuning_status_is_refused_when_a_forwarded_header_arrives` |
| R-11 | Container limit below host memory; "no limit" read as a number | `the_cgroup_limit_wins_over_host_memory`, `a_cgroup_unlimited_sentinel_is_unknown_not_a_limit` |
| R-12 | GPU says valid, CPU says invalid | `gpu_verdict_never_reaches_the_verified_cache` and `a_block_with_a_gpu_accepted_bad_signature_is_refused` (fake backend that always says yes) |
| R-13 | GPU says invalid, CPU says valid | `a_gpu_refused_valid_transaction_is_still_admitted_and_mined` (fake backend that always says no) |
| R-13a | v7 or v8 frame handed to the GPU; one long dispatch resets the device | `suite_tagged_and_multisig_frames_never_reach_the_gpu`, `a_gpu_batch_is_cut_into_bounded_dispatches` |
| R-14 | First mismatch | `one_mismatch_latches_the_gpu_off_for_the_process` |
| R-15 | Crafted signatures that hit edge encodings | G0 differential suite; mutations listed in the exit test |
| R-16 | Hybrid with one good half | `the_gpu_reports_each_half_and_never_a_hybrid_verdict` |
| R-17 | Job names an unlisted runtime, carries code, or names a path outside scratch; the runtime tries to read the key | `a_job_outside_the_signed_allow_list_is_never_started`, `a_job_input_naming_a_path_outside_scratch_is_refused`, `an_inference_runtime_cannot_read_the_worker_key` |
| R-18 | Job arrives while tuned or over temperature | `a_job_is_refused_unless_power_and_clocks_read_back_at_default`, `no_job_is_taken_during_a_thermal_hold` |
| R-19 | Many jobs to exhaust the bond or to cycle the card | `bond_exposure_never_passes_the_users_cap`, `the_scheduler_never_cycles_the_card_faster_than_its_dwell` |
| R-20 | Manifest claims a card the worker lacks; self-run calibration read as proof | `a_device_class_is_labelled_declared_or_calibrated_self_run_never_verified`, `calibration_never_raises_the_bond_cap` |

## Build order and what each step is

| Step | Item | Class at end | Node binary | PR |
|---|---|---|---|---|
| 1 | HW-1 T0 efficiency read-out, absolute tool path, miner `hw_error` event | REAL | no | normal |
| 2 | HW-1 T1a guided tuner: state machine, journal, UUID keys | REAL (NVIDIA); AMD/Intel PLANNED | no | normal |
| 2b | HW-1 T1b helper and automatic mode | REAL | no | **gated** on the helper conditions (signing key, per-machine install) and O-2 |
| 3 | HW-2 probe, profile, override, status, cross-profile test | REAL | yes | `[post-genesis]` draft |
| 4 | HW-3 G0 `hal/wgpu-verify` and harness | RESEARCH | no | normal |
| 5 | HW-3 G1 shadow beside the builder prewarm, only after step 4's exit test | RESEARCH | yes | `[post-genesis]` draft |
| 6 | HW-4 scheduler and Compute page against a fake worker | SIM (says so) | no | normal |
| 7 | HW-4 against ADR-129's worker | REAL when ADR-129 is | no | after ADR-129 |

Why this order: HW-1 touches no frozen code and gives the telemetry HW-4's
scheduler needs; HW-2 creates the profile and status field HW-3 reports
through; HW-3 G0 is independent but its node half needs HW-2; HW-4 needs
ADR-129. The per-step files, red tests and commands are in
`.claude/plans/hardware-intelligence.plan.md` (untracked). Every cargo
command goes through the slot, one scope at a time, for example
`bash /d/Temp/claude-locks/cargo-slot.sh hw1-tuner -- cargo test --manifest-path apps/hub/src-tauri/Cargo.toml tuner 2>&1 | tail -25`
(the Hub is its own workspace) and
`bash /d/Temp/claude-locks/cargo-slot.sh hw3-g0 -- cargo test -p maya-wgpu-verify 2>&1 | tail -25`.

**SIM and REAL.** No simulator ships. The test doubles (`FakeGpuControl`,
the always-yes and always-no verify backends, the fake worker) are
`#[cfg(test)]` or a named dev binary that prints that it is a fake. HW-4
step 6 is the one SIM deliverable and is labelled in the ledger.

## Three more items, ranked

Proposed, not started; each needs Eric's yes (O-9).

| Rank | Item | Need | Rung | Risk | Prior art |
|---|---|---|---|---|---|
| 1 | **HW-5 Entropy health in key tools.** Wire `hal/entropy`'s pool and its SP 800-90B health results into key generation in the wallet, Hub and node key commands, and show a failed source to the user. | Key generation is where a silent hardware fault costs everything, and the crate is verified but no other manifest depends on it. | Now; not consensus; key tools only | Low. A false alarm blocks key generation, which fails safe. SIM sources stay behind their feature. | NIST SP 800-90B continuous health tests; FIPS 140-3 entropy requirements; Linux kernel RNG health tests |
| 2 | **HW-7 Energy-aware mining schedule.** The Hub mines only inside hours or under a price the user enters; later, a read-only meter value through `hal/energy`'s Modbus parser. | Lowers a miner's cost with a visible, opt-in rule; reuses HW-1's efficiency read-out. | After HW-1; Hub only | Medium. A LAN meter read is a new network surface in a desktop app: user-entered address only, read function codes only, no scanning. `hal/energy`'s market and surge modules are SIM and are not used. Savings are MISSING until measured. | Mining managers with time schedules (NiceHash, HiveOS); demand-response programmes used by large miners |
| 3 | **HW-6 Adaptive XDP filter.** A user-space controller adjusts `hal/ebpf-net`'s rate and block maps from its drop counters. | Cheap drop of floods before the kernel allocates. | Post-mainnet; Linux only; the relay is RESEARCH and must be promoted first | High. UDP sources are spoofable, so an adaptive block list is a tool for making a node block honest peers. Needs expiry on every entry, a floor it cannot go under, and never blocking an address learned over an authenticated session. | Cloudflare L4Drop, Meta Katran, Cilium (XDP filtering and load balancing) |

**Flagged, not designed: TEE or vendor hardware attestation.** It would give
HW-4 a real device class and a fourth verification rung, and it collides
with the recorded invariants (a vendor key is a trusted party; the keys are
not post-quantum). It stays blocked until that conflict is reconciled in
its own ADR.

## Alternatives considered

- **A learned model for tuning (HW-1, HW-2).** A bounded step search and a
  table are testable as pure functions and explainable to a user; a model is
  neither, for no measured gain.
- **Overclocking in the first tuner.** Raising clocks or power is where
  instability and hardware wear come from; lowering the power limit is the
  lever with a safe direction.
- **Run the Hub elevated, or install a tuning service.** A larger attack
  surface and the shape store and antivirus reviews reject.
- **Ship the automatic tuner now with a helper pinned like the miner.** The
  miner pin stages a copy in a user-owned folder; elevating that copy is a
  path to root for any user-level process (S-1).
- **Reorder mempool verification by GPU answer (the draft's G2).** Removed:
  see the staging table.
- **Tune the node from benchmarks at start-up.** A start-up benchmark is
  wall-clock dependent and slow; a table keyed on probed facts is the same
  on every start.
- **CUDA for verification (cuPQC-style).** One vendor, a closed library on a
  node path, and invariant 3's reason. wgpu matches the existing miner.
- **Trust the GPU after N clean shadow comparisons.** A count of past
  agreements does not bound the next answer.

## Consequences

- The guided tuner changes nothing on the machine. The Hub's first write to
  hardware and its first elevated helper arrive only with step 2b, behind
  conditions that include a signing key the project does not have yet.
- With G2 removed, HW-3 inside the node is measurement only. Its value is
  the G0 numbers and the evidence for or against a later G3 ADR.
- `gpu.rs` moves from `Command::new("nvidia-smi")` (a `PATH` search) to an
  absolute path in step 1, which is a fix on its own.
- The node gains a `[tuning]` config section. `NodeConfig` sections reject
  unknown fields, so a config with `[tuning]` does not load on an older
  binary; the rollout note says to add it after upgrading.
- `features.toml` gains four `planned` entries; `architecture-vision.md` §4
  gains four rows. No invariant is added: none is earned until a test exists.
- HW-3 may end at G0 with "not worth it". That is recorded as a result.

## Security-architect pass (adversarial read of this ADR, 2026-11-12)

A read-only `security-architect` review of the first draft and the four
threat models, checked against the code. It found three statements about
existing code that were wrong, and nineteen design holes. All are folded
into the text above; this table is the record.

**Wrong statements corrected.** (a) The draft said the sequential pass
"still calls `verify_at` for every transaction"; in fact every check goes
through `VerifiedCache::verify`, and a hit returns without `verify_at`. (b)
The draft said prewarm runs on a block's transactions; it runs on the
builder's candidates (`db.rs:1124`) and never in block application, so the
draft's single cross-profile test would not have exercised the knob. (c)
The HW-1 threat model said the miner re-checks GPU results on the CPU; it
does not today, and the `hw_error` event does not exist. It is plan step 1.

| # | Sev. | Hole | Fix applied |
|---|---|---|---|
| S-1 | CRITICAL | HW-1: on Linux the helper would have been elevated from the staged copy in a user-owned temp folder (`miner_guard.rs` staging); any user-level process could swap it after the hash check and run as root. | Automatic mode gated; helper only from an admin-only path, owner and mode checked, polkit action bound to the path. |
| S-2 | HIGH | HW-1: on Windows the pin protects a handle, the install is per-user (to be confirmed), and no release signing key exists; an unsigned elevated binary also draws "unknown publisher" prompts and antivirus heuristics. | Per-machine install and Authenticode signer check are conditions for step 2b. |
| S-3 | HIGH | HW-1: "the helper exits" and "one prompt per session" contradicted each other. | One resident helper per session, one authenticated channel, lifetime cap, default-on-exit. |
| S-4 | HIGH | HW-1: the journal failed open: a deleted or edited journal skipped rollback. | Separate `trying` marker; unreadable, missing or unknown means trial in progress. Residual: user-level malware that rewrites both files consistently can clear a bad mark; the envelope still bounds every value. |
| S-5 | HIGH | HW-1: a thermal abort restored the previous, higher limit on a hot card. | Hold the lower limit until under ceiling minus hysteresis. |
| S-6 | HIGH | HW-1: only `temperature.gpu` is read; memory junction heat is often the limit on mining cards. | Memory temperature is read and used where the driver reports it. **Partly accepted:** it is not a precondition for T1, because T1 only lowers power; it is a precondition for T2 and T3. |
| S-7 | HIGH | HW-1: device index can name different cards in different tools. | UUID everywhere, resolved before each write. |
| S-8 | HIGH | HW-1: on Windows a tool planted beside the Hub runs before `PATH` is consulted. | Absolute system path, no search, signer check before any elevated use; fixed in step 1 for the read path. |
| S-9 | HIGH | HW-2: loopback is not access control; the public tunnel is on the same host. | Method absent by default; refuses forwarding headers. |
| S-10 | HIGH | HW-3: the G3 sketch said a wrong GPU value "fails closed"; on a consensus path a false refusal is a split, and the sketch let the GPU supply the SLH-DSA digest and indices. | G3 rewritten as an open question with those two problems stated. |
| S-11 | HIGH | HW-3: G2 assumed a verification queue that does not exist; with fee eviction a delayed valid transaction could be dropped in a flood. | G2 removed from this ADR. |
| S-12 | MEDIUM | HW-3: a long dispatch can trip the GPU watchdog and reset a card the miner shares; the "disable while tuned" rule needed a channel that does not exist. | Bounded dispatches; default off; operator rule; latch as backstop. |
| S-13 | MEDIUM | HW-4: no process boundary between a runtime parsing strangers' data and the worker key. | Child process without key, network or outside paths; signing elsewhere. |
| S-14 | MEDIUM | HW-4: calibration is self-runnable and its label drifted towards "verified". | Fixed label; never raises the bond cap. |
| S-15 | MEDIUM | HW-4: "driver defaults" conflicted with "restore original". | Read-back at driver default is the test; otherwise the job is refused. |
| S-16 | MEDIUM | HW-4/HW-1: no minimum dwell, so cheap jobs could cycle the card. | Dwell per state and a writes-per-hour cap. |
| S-17 | MEDIUM | HW-1 T2: a pinned fan that outlives a crash. | Every exit releases to automatic; T2 stays design-only. |
| S-18 | MEDIUM | HW-2: cgroup "no limit" values read as numbers. | Parsed as `Unknown`. |
| S-19 | LOW | HW-2: "threads below the core count" is impossible on one core; builder and apply need separate tests. | Tests split and reworded. |

Also corrected from the review: the invariant rows for 2, 29, 31 and 33.
**Not verified by the review or by this ADR:** Tauri's default install mode
for the Hub, the enumeration order of CUDA against `nvidia-smi`, the length
of the Windows GPU watchdog window, and whether an `nvidia-smi` power limit
persists across a reboot. Each is a build-lane check before the step that
depends on it.

## Open questions for Eric

Each has a recommended option, which is also the default if unanswered.

| # | Question | Options | Recommended |
|---|---|---|---|
| O-1 | HW-1 levers in the first release | **A** power limit only, downward · **B** A plus fan (upward only) · **C** B plus clock offsets | **A**. B after A is measured. C needs its own consent text and a driver-capability study. |
| O-2 | How HW-1 applies a limit | **A** guided now (the Hub suggests, you set it, the Hub measures; no admin rights), automatic later through a signed helper once the release signing key and a per-machine install exist · **B** automatic now with an unsigned helper · **C** guided only, never a helper | **A**. Never B: it is the one critical finding of the security pass. |
| O-3 | HW-1 hash-rate floor during tuning | **A** fixed: keep at least 95 % of baseline · **B** fixed 90 % · **C** a slider, default 95 % | **C**. The percentages are defaults to confirm, not measurements. |
| O-4 | HW-2 default mode when it ships | **A** `off`, opt-in until a before/after bench exists, then `auto` · **B** `auto` for everyone · **C** `auto` for non-validators, `off` for validators | **A**. |
| O-5 | HW-2 probing on Windows (memory and disk type need an OS call) | **A** standard library only; Windows memory and disk are `Unknown` and keep today's defaults · **B** add the `sysinfo` crate to the node binary (a core dependency: needs your approval and a `cargo vet` entry) · **C** hand-written OS calls with `unsafe` | **A** first; **B** if a bench shows Windows nodes lose from it. |
| O-6 | HW-3: may a GPU answer ever stand without a full CPU check | **A** no; CPU authoritative; stop at G1 · **B** study the hash-bound split in its own ADR after G0 numbers exist · **C** trust GPU accepts after differential tests | **A** now, **B** only if G0 shows a large measured gain. Never C. |
| O-7 | HW-3 scope to approve now | **A** G0 only, decide G1 on its numbers · **B** G0 and G1 as written · **C** drop HW-3 | **A**. |
| O-8 | HW-4 device class | **A** declared plus calibration evidence, never called attested · **B** wait for a TEE decision · **C** no device class | **A**. |
| O-9 | The three extra items | **A** HW-5 now, HW-7 after HW-1, HW-6 post-mainnet · **B** HW-5 only · **C** none | **A**. |
| O-10 | HW-4 what a Hub worker may run | **A** only runtimes and models on a list signed with the Hub release · **B** any job the chain accepts | **A**. Running strangers' prompts also raises a content and liability question that needs a lawyer, not this ADR. |
