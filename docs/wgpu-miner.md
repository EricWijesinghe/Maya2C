# The wgpu GPU miner

Cross-platform GPU mining for hashimoto, via WGSL compute shaders.

---

## Why hashimoto and not ArgonBlake

`cuda-miner` already accelerates ArgonBlake, the proof of work before
`DAG_ACTIVATION_HEIGHT`. This crate does not duplicate it, and the reason is
about WGSL rather than about effort.

**WGSL has no native 64-bit integer type.** Argon2id and BLAKE2b are u64
algorithms throughout — `cuda-miner/src/argon2_ref.rs` uses `u64` in eighteen
places. Emulating u64 as pairs of u32 through BLAKE2b's G function is possible,
slow, and a correctness surface on a *consensus hash*, where one wrong bit
produces blocks the network rejects silently and only once a solution is found.
CUDA has native u64. That port belongs there.

Hashimoto is the opposite case:

| | ArgonBlake | Hashimoto |
|---|---|---|
| Arithmetic | u64 (BLAKE2b) | **`[u32; 32]` mix, FNV only** |
| Bound by | VRAM capacity, 32 MiB/lane | **memory bandwidth** |
| wgpu fit | poor | **good** |

`crypto::dag::hashimoto`'s own documentation: *"the arithmetic between reads is
a handful of multiply-xors — deliberately trivial, so that the DRAM round trip
is the entire cost and the hash rate a machine achieves is a direct reading of
its memory bandwidth."* That is exactly the shape a GPU helps with.

## The split

```text
host   seed = BLAKE3-512(mix_key, header_hash ‖ nonce_le)     crypto::dag::hashimoto::seed_for
GPU    mix  = 64 data-dependent FNV accumulations             shaders/hashimoto.wgsl
host   result = BLAKE3(mix_key, seed ‖ compress(mix))         crypto::dag::hashimoto::finish
```

**No hash is reimplemented.** Both BLAKE3 calls are the node's own.

An earlier draft of `reference.rs` reproduced the seed derivation from a guessed
domain string — the same class of error that produced unspendable addresses in
the browser SDK. The fix was to expose the seam from the node instead:
`hashimoto::seed_for` and `hashimoto::finish` are now `pub`, with the reasoning
recorded there. One BLAKE3 on the consensus path, not one per backend.

The split costs almost nothing: a seed is 64 bytes uploaded per nonce against
8 KiB of device traffic the loop it feeds will move — under one percent.

## The dataset is chunked, and the limit is measured

The mainnet dataset is 4 GiB. wgpu's `maxStorageBufferBindingSize` is far below
that on every backend, so the dataset is split across four bindings — WGSL has
no array-of-bindings, so they are declared individually and selected by a switch
in `read_word`.

Chunk boundaries fall on page boundaries, so no page ever straddles two buffers.
A straddling page would need the shader to stitch a read across bindings, which
is the kind of code that works on one backend.

**An adapter that would need more than four chunks is refused at startup.**
Silently mining a truncated dataset would produce digests that differ from the
node's, and the miner would look like it was working until the first solution
was rejected.

`wgpu-miner --list` reports each adapter's real limits and the chunk count a
4 GiB dataset would need on it. Run it before assuming anything. Measured on
the development host:

```
idx adapter                                backend      max bind
0   NVIDIA GeForce RTX 5070 Laptop GPU     Vulkan         2047 MiB
1   NVIDIA GeForce RTX 5070 Laptop GPU     Dx12           2047 MiB
2   Microsoft Basic Render Driver          Dx12           2047 MiB
3   NVIDIA GeForce RTX 5070 Laptop GPU/PCIe/SSE2  Gl      2047 MiB

Chunks needed for the 4 GiB mainnet dataset (shader declares 4):
  [0] 3 — ok    [1] 3 — ok    [2] 3 — ok    [3] 3 — ok
```

Three chunks against four declared. That is the margin the constant was chosen
for, and it is now a measurement rather than an estimate.

## The `gpu` feature is off by default

Same rule as `cuda-miner`'s `cuda` feature, and invariant 3's reason:
`cargo build --workspace` must stay green on a runner with no adapter and no
graphics driver. With no feature selected the crate is the host pipeline and the
CPU reference — which is what the parity tests compare against anyway — and the
binary says plainly that it cannot mine.

```
cargo build -p maya-wgpu-miner --features gpu
```

## Validation

`wgpu-miner/tests/gpu_validation.rs`, layered so a failure says *where*:

| Layer | Compares | Runs without a GPU |
|---|---|---|
| 1. Seam | seed words against the node's seed bytes | yes |
| 2. CPU mix | `mix_on_cpu` + `finish` against `hashimoto_light` | yes |
| 3. GPU mix | `hashimoto.wgsl` against `mix_on_cpu` | no |
| 4. End to end | GPU digest against `hashimoto_light` | no |

Layer 2 matters more than it looks: without it, layer 3 would prove only that
two wrong things agree.

The GPU layers are `#[cfg(feature = "gpu")]` — **skipped, not failed**, when the
feature is off. A test that failed for want of hardware is a test people learn
to ignore.

### Run them under nextest

```
cargo nextest run -p maya-wgpu-miner --features gpu     # 10 passed
```

**`cargo test` hangs on this binary.** Every test passes alone; run together,
the harness executes them on parallel threads in one process, and creating and
dispatching several wgpu devices concurrently deadlocks on this driver. nextest
gives each test its own process and the problem does not arise — which is
convenient, since nextest is already this repository's primary runner.
`cargo test -- --test-threads=1` also works.

A shared device behind a `OnceLock<Mutex<..>>` was tried and did **not** fix it:
the deadlock is in device creation and dispatch, not in the dataset. It also
raised an `overflow evaluating ... : Sync` warning slated to become a hard
error, so it was removed rather than kept with a comment claiming a fix it did
not deliver.

`a_partial_workgroup_is_computed_correctly` runs batches of 1, 63, 65, and 127
against a workgroup size of 64. The dispatch rounds up to whole workgroups and
the shader returns early past `nonce_count`; those sizes are where an off-by-one
in that guard shows up, as either a wrong digest or a buffer overrun.

## Not implemented

**Work distribution.** The binary enumerates adapters and runs the validated
compute path. It does not poll `get_mining_candidate` or submit blocks, and it
says so and exits rather than pretending. The compute half is the half that
needed proving; wiring it to an RPC endpoint is ordinary plumbing that should be
written against a running node, not against an assumption about one.

**Backends other than the one tested here.** wgpu targets Vulkan, DX12, Metal,
and GL. Only the adapter on this build host was exercised. The shader is
portable by construction — u32 only, no extensions — but portable-by-construction
is an argument, not a test result.

**Mainnet-sized datasets.** Validation runs against `Params::TESTING`, a 4 MiB
dataset. The algorithm is identical at mainnet sizes; the memory behaviour is
not, and a 4 GiB dataset on a 4 GiB laptop adapter will not fit alongside a
framebuffer. Measure before promising a hash rate.
