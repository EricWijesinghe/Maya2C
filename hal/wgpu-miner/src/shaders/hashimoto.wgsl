// The hashimoto mix loop, in WGSL.
//
// ---------------------------------------------------------------------------
// What is on the GPU, and what deliberately is not
// ---------------------------------------------------------------------------
//
// Only the accumulation loop. Both BLAKE3 calls that frame it — the seed
// derivation and the final squeeze — stay on the host, which is the same
// discipline `cuda-miner` follows: port the cost centre, reimplement nothing
// else.
//
// That split costs almost nothing. A seed is 64 bytes uploaded per nonce; the
// loop it feeds moves ACCESSES x 128 = 8 KiB of device memory per nonce. The
// upload is under one percent of the traffic, and in exchange BLAKE3 has
// exactly one implementation on the consensus path instead of two.
//
// ---------------------------------------------------------------------------
// Why this maps to WGSL at all
// ---------------------------------------------------------------------------
//
// Every value here is a u32. The mix is `array<u32, 32>`, an item is
// `array<u32, 16>`, and the only arithmetic is FNV — a 32-bit multiply and an
// xor.
//
// That is not luck, it is why this algorithm was chosen over ArgonBlake for a
// wgpu port. WGSL has no native 64-bit integer type, and Argon2id and BLAKE2b
// are u64 throughout. Emulating u64 as u32 pairs inside a consensus hash is a
// correctness surface nobody should want.
//
// ---------------------------------------------------------------------------
// The dataset is chunked
// ---------------------------------------------------------------------------
//
// The mainnet dataset is 4 GiB and wgpu's `maxStorageBufferBindingSize` is far
// below that on every backend. So the dataset arrives as CHUNKS bindings and
// `read_word` resolves a global word index into (chunk, offset).
//
// The chunk count is a host-side decision made from the adapter's reported
// limits — see `probe.rs`. This shader is written against whatever it is told.

// 128-byte mix, as 32 words.
const MIX_WORDS: u32 = 32u;
// 64-byte dataset item, as 16 words.
const ITEM_WORDS: u32 = 16u;
// One page is two items: 32 words.
const PAGE_WORDS: u32 = ITEM_WORDS * 2u;
// Dataset reads per hash. Must equal `custom_l1_node::crypto::dag::ACCESSES`.
const ACCESSES: u32 = 64u;
// Ethash's FNV prime.
const FNV_PRIME: u32 = 0x01000193u;

struct Job {
    // Pages in the dataset. The lookup index is reduced modulo this.
    pages: u32,
    // Nonces this dispatch covers. A workgroup past this does nothing, so the
    // host may round the dispatch up to a whole number of workgroups without
    // computing digests nobody asked for.
    nonce_count: u32,
    // Words per chunk. Constant across chunks except possibly the last.
    chunk_words: u32,
    // Chunks actually populated, so a small dataset does not require the host
    // to bind CHUNKS real buffers.
    chunk_count: u32,
};

@group(0) @binding(0) var<uniform> job: Job;

// Seeds, 16 words each (the 64-byte BLAKE3-512 output), one per nonce.
@group(0) @binding(1) var<storage, read> seeds: array<u32>;

// Output mixes, MIX_WORDS each, one per nonce.
@group(0) @binding(2) var<storage, read_write> mixes: array<u32>;

// The dataset, split across fixed bindings. WGSL has no array-of-bindings, so
// the chunks are declared individually and selected by a switch in `read_word`.
// Four is enough for a 4 GiB dataset against a 1 GiB binding limit, and the
// host refuses to launch if it needs more.
@group(0) @binding(3) var<storage, read> chunk0: array<u32>;
@group(0) @binding(4) var<storage, read> chunk1: array<u32>;
@group(0) @binding(5) var<storage, read> chunk2: array<u32>;
@group(0) @binding(6) var<storage, read> chunk3: array<u32>;

// FNV: a 32-bit wrapping multiply and an xor.
//
// WGSL integer arithmetic wraps by definition, which is what the Rust side
// spells `wrapping_mul`. The two agree without any explicit masking.
fn fnv(a: u32, b: u32) -> u32 {
    return (a * FNV_PRIME) ^ b;
}

// Reads one word of the dataset by global word index.
//
// Bounds are clamped rather than trusted. A shader that indexed past a binding
// would return an implementation-defined value on some backends and trap on
// others, and a miner that produced a different digest depending on which GPU
// ran it is worse than one that produces none.
fn read_word(index: u32) -> u32 {
    let chunk = index / job.chunk_words;
    let offset = index % job.chunk_words;

    switch chunk {
        case 0u: { return chunk0[min(offset, arrayLength(&chunk0) - 1u)]; }
        case 1u: { return chunk1[min(offset, arrayLength(&chunk1) - 1u)]; }
        case 2u: { return chunk2[min(offset, arrayLength(&chunk2) - 1u)]; }
        case 3u: { return chunk3[min(offset, arrayLength(&chunk3) - 1u)]; }
        default: { return 0u; }
    }
}

// One nonce per invocation.
//
// The workgroup size is 64: one nonce per invocation, and 64 is a multiple of
// both a 32-wide NVIDIA warp and a 64-wide AMD wavefront, so no lane is idle
// on either. It is overridable from the host and reported by the CLI, because
// the best value is a property of the adapter rather than of the algorithm.
@compute @workgroup_size(64)
fn mine(@builtin(global_invocation_id) gid: vec3<u32>) {
    let nonce_index = gid.x;
    if (nonce_index >= job.nonce_count) {
        return;
    }

    // The mix starts as the 16-word seed laid down twice, so it is one page
    // wide and a lookup folds in without reshaping.
    var mix: array<u32, 32>;
    let seed_base = nonce_index * ITEM_WORDS;
    for (var i = 0u; i < MIX_WORDS; i = i + 1u) {
        mix[i] = seeds[seed_base + (i % ITEM_WORDS)];
    }

    let seed0 = seeds[seed_base];

    for (var access = 0u; access < ACCESSES; access = access + 1u) {
        // Data-dependent: the mix decides where the next read lands, so the
        // reads cannot be batched, reordered, or prefetched. That dependency
        // is the whole design — it is what makes the hash rate a reading of
        // memory latency and bandwidth rather than of shader count.
        let page = fnv(access ^ seed0, mix[access % MIX_WORDS]) % job.pages;
        let word_base = page * PAGE_WORDS;

        for (var w = 0u; w < MIX_WORDS; w = w + 1u) {
            mix[w] = fnv(mix[w], read_word(word_base + w));
        }
    }

    // The mix goes back to the host uncompressed. Folding it to 8 words here
    // would save 96 bytes per nonce of readback and move a second piece of
    // consensus arithmetic onto the GPU; the host already walks the result to
    // run BLAKE3 over it, so it costs nothing to fold there.
    let out_base = nonce_index * MIX_WORDS;
    for (var w = 0u; w < MIX_WORDS; w = w + 1u) {
        mixes[out_base + w] = mix[w];
    }
}
