# The DAG proof of work

A memory-hard, dataset-bound proof of work that replaces ArgonBlake at
`DAG_ACTIVATION_HEIGHT`. Structurally it is Ethash: a small **cache** every node
holds, and a large **dataset** derived from it that only miners hold. A hash
reads pseudo-random pages of the dataset, so the work is bound by memory
bandwidth rather than by arithmetic.

| | |
|---|---|
| Dataset | 4 GiB, fixed size, contents rotate every epoch |
| Cache | 64 MiB, what a validating node holds |
| Epoch | 30,000 blocks — 5.21 days at a 15-second target |
| Lookups per hash | 64 pages of 128 bytes |
| Hash | BLAKE3 throughout |
| Activation | Height 30,000, the start of epoch 1 |

## What this does and does not claim

It does **not** make general-purpose hardware beat purpose-built silicon.
Nothing has ever achieved that, and the design this one is modelled on is the
proof: Ethash was the canonical ASIC-resistant DAG, and Ethash ASICs shipped in
April 2018 at roughly 2–2.5× the efficiency of contemporary GPUs. The mechanism
is structural. When an algorithm is bound by commodity DRAM bandwidth, an ASIC
buys the *same* commodity DRAM and then deletes the display engine, the shader
array, the PCIe complex and the driver stack.

What a DAG buys is a **ceiling on that advantage**. A SHA-256 ASIC beats a CPU
by roughly four orders of magnitude because the work is pure arithmetic and
arithmetic is what custom silicon is best at. Here the work is DRAM traffic,
which custom silicon has to buy on the same open market as everyone else. The
realistic outcome is a 2–5× gap rather than a 10,000× one — the difference
between GPU mining being worthless and GPU mining being viable. That is the
claim this design makes, and it is the only one it can support.

### Tuned for GDDR6/6X/7, not HBM

Deliberately targeted at 400–1000 GB/s commodity graphics memory. Tuning for
HBM3 would favour datacenter accelerators — the most centralized and least
commodity hardware there is — which is the opposite of the point. The 128-byte
page is the largest fully-utilised transaction a GDDR memory system offers; a
wider page would start to reward the memory systems only HBM parts have.

### Fixed size, rotating contents

Ethash grew its dataset every epoch, which is what eventually made 4 GB cards
unable to mine Ethereum at all. Here the size is constant and only the
*contents* rotate. A card that can mine today can mine indefinitely, and the
hardware floor is a published constant rather than a moving deadline.

## Construction

```text
seed(0)    = H(context)
seed(n)    = H(seed(n-1))

cache[0]   = H(seed(epoch))
cache[i]   = H(cache[i-1])
repeat 3 times:
    for i: cache[i] = H(cache[(i-1) mod n] XOR cache[cache[i][0] mod n])

item(i)    = H( fold 256 pseudo-random cache items into H(cache[i mod n] XOR i) )

hashimoto(header, nonce):
    seed = H(header_with_nonce_zeroed || nonce)      # 64 bytes
    mix  = seed repeated to 128 bytes
    64 times:
        page = fnv(access XOR seed[0], mix[...]) mod pages
        mix  = fnv(mix, dataset[page])               # 128 bytes
    return H(seed || compress(mix))
```

Everything hashes with BLAKE3 under four distinct derived keys — epoch seed,
cache item, dataset item, hashimoto mix — so no two stages can collide even on
identical input. Keys are derived once and used in keyed mode; `derive_key`
re-hashes its context string on every call, and dataset generation makes 10⁸ of
them.

The cache and page counts are rounded **down to a prime**. A composite count
shares factors with the strides the mixing function produces, which lets the
walk fall into cycles far shorter than the array — the cache would be nominally
64 MiB and effectively a fraction of it, and every test would still pass.

## The asymmetry that makes it usable

A dataset item is a pure function of the cache and its index. A miner
materialises all 67 million of them; a validator recomputes only the ~128 a
block actually touched.

Measured on this codebase (`cargo bench --bench dag`, release):

| | Cost |
|---|---|
| Validate one block (`hashimoto_light`, mainnet) | **2.0 ms** |
| Validate one block under ArgonBlake, the rule this replaces | ~25.4 ms |
| Recompute one dataset item | 5.0 µs |
| Mine one hash from a materialised dataset (test sizes, CPU) | 2.3 µs |
| Mine one hash from the cache (test sizes, CPU) | 600 µs |
| Generate an epoch's 64 MiB cache | 0.93 s |

Two things follow. Validation got **an order of magnitude cheaper** at the same
time as mining became bandwidth-bound. And mining from the cache costs ~260×
mining from the dataset, so holding the 4 GiB is the only sane strategy — which
is what makes the memory requirement real rather than nominal.

The cache is regenerated once per epoch, against a 5.21-day budget. The node's
`dag_prepare_loop` builds the next epoch's cache 100 blocks — about 25 minutes —
before the boundary, so the first block of a new epoch never pays a ~0.9 s
generation inside `insert_block` under the chain lock. Two epochs are held at a
time, so a reorg across a boundary does not regenerate inside fork choice.

## The fork

Both rules stay in the binary permanently: a node syncing from genesis has to be
able to check the chain as it was.

* Below `DAG_ACTIVATION_HEIGHT`: ArgonBlake, unchanged, frozen known-answer test
  intact.
* At and above: hashimoto against the epoch's cache.

The rule is selected by the height the block lands at, which the validator
derives from the parent — never from anything the block declares. A miner cannot
select an easier rule by claiming to be somewhere else in the chain, and the
header needs no new field, so `HEADER_LEN` is unchanged and the nonce stays
where the GPU miner and the SV2 share path expect it.

### Difficulty is reset at the fork block

The target in force just before the fork was calibrated against a 25 ms hash.
The rule replacing it costs ~2 ms on the same CPU and microseconds on a GPU.
Carrying that target across would let a retarget window be mined in seconds, and
retargeting moves by at most 4× per window.

So the fork block takes `DagConfig::activation_target` instead of inheriting,
and the default is the network's **easiest** permitted target. The two ways of
being wrong are not symmetric: too easy means blocks arrive too fast for a few
windows, which retargeting fixes on its own; too hard means the chain stops, and
a stopped chain cannot retarget its way out.

## On the GPU

`cuda-miner` holds one 4 GiB dataset that every lane reads. This is a different
shape of constraint from ArgonBlake, which pinned 32 MiB *per in-flight hash* —
an 8 GiB card ran ~225 lanes and capacity was the ceiling. Now thousands of
lanes run against one allocation and the ceiling is bandwidth, which is the
resource this design intends to be scarce.

* The dataset is generated **on the device** from the uploaded 64 MiB cache. A
  4 GiB PCIe transfer takes longer than the generation does.
* A page is read as eight `uint4` loads: four 32-byte sectors, all wanted.
* One thread per nonce, 64 dependent reads each. A thread is latency-bound by
  construction; throughput comes from thousands of them in flight, which is what
  converts memory latency into memory bandwidth.
* VRAM floor is ~6 GB.

### How the kernel is kept correct without a GPU in CI

A CUDA kernel is the hardest code here to test, so the port happens in two
steps:

1. `hal/cuda-miner/src/blake3_ref.rs` and `hal/cuda-miner/src/dag.rs` implement exactly
   what the kernel implements — including BLAKE3's compression function, flag
   schedule and padding rules — in Rust, checked byte-for-byte against the
   `blake3` crate and against the node on ordinary hardware.
2. `hal/cuda-miner/kernels/dag.cu` is a transliteration of code that is already
   known to be correct.

The remaining gap — whether the transliteration is faithful — is what the
`--features cuda` tests in `hal/cuda-miner/tests/dag_parity.rs` close, and they need
a card. **They have not been run: this tree has no CUDA toolkit and no GPU.**
The `cuda` feature is off by default, `build.rs` never invokes nvcc without it,
and nothing links against the CUDA runtime, so a GPU-less build stays green.

## Tests

| Where | What |
|---|---|
| `crates/node/src/crypto/dag/*` unit tests | Determinism, thread-count independence, light/full agreement, epoch rotation, lookup distribution |
| `crates/node/tests/dag_tests.rs` | Frozen vectors at test **and mainnet** sizes; epoch selection; chain acceptance and rejection; the fork's difficulty reset |
| `hal/cuda-miner/tests/dag_parity.rs` | The miner's independent implementation against the node's, item for item, and both against the frozen vectors |
| `hal/cuda-miner/tests/dag_parity.rs` (`--features cuda`) | The kernel against the CPU reference. Needs a GPU |
| `crates/node/tests/fixtures/dag_vectors.json` | The bytes themselves. Regenerating it is a hard fork — see `crates/node/examples/gen_dag_vectors.rs` |

Test-sized parameters (`Params::TESTING`: 512 KiB cache, 4 MiB dataset) exist so
that whole-dataset determinism can be asserted on a CI runner rather than only
on someone's workstation behind `--ignored`. The mainnet vectors are checked
too, and cost one 64 MiB cache and no dataset at all — a dataset item is
computable from the cache alone, which is the same asymmetry a validator relies
on.
