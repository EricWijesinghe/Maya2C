// Maya2C DAG proof of work, on the device.
//
// ## What this file is, and what it is not
//
// It is a transliteration. Every function here has a Rust counterpart that is
// byte-checked against the node on ordinary CI hardware:
//
//   blake3_compress   <-  src/blake3_ref.rs::compress
//   keyed_512 / _256  <-  src/blake3_ref.rs::keyed_512 / keyed_256
//   dataset_item      <-  src/dag.rs::dataset_item
//   hashimoto         <-  src/dag.rs::hashimoto_full
//
// It is *not* an independent implementation, and it must never become one. A
// change here that is not made there — or vice versa — produces a miner that
// grinds valid-looking work the network rejects. When something needs to
// change, change the Rust first, let `tests/dag_parity.rs` prove it against the
// node, then port the diff.
//
// ## Why the work is laid out this way
//
// One thread per hash, each doing ACCESSES dependent 128-byte reads. The reads
// are the entire cost: sixteen multiply-xors of arithmetic sit between them, so
// a thread spends essentially all of its life waiting on DRAM. That is the
// design goal, not a defect — occupancy exists to hide that latency behind
// thousands of other threads doing the same thing, which is what converts
// memory *latency* into memory *bandwidth* and makes the hash rate a direct
// reading of the card's GB/s.
//
// A 128-byte page is read as eight uint4 loads: four 32-byte sectors, all of
// them wanted, which is the largest fully-utilised transaction the memory
// system offers.
//
// ## Building
//
// `build.rs` compiles this with nvcc under the `cuda` feature, which is off by
// default so that GPU-less CI still builds a green workspace.

#include <cuda_runtime.h>
#include <stdint.h>
#include <stdio.h>

// ---------------------------------------------------------------------------
// protocol constants — must equal src/dag.rs
// ---------------------------------------------------------------------------

#define ITEM_WORDS 16
#define MIX_WORDS 32
#define DATASET_PARENTS 256u
#define ACCESSES 64u
#define FNV_PRIME 0x01000193u

// ---------------------------------------------------------------------------
// BLAKE3 — the single-chunk, keyed slice of it this protocol uses
// ---------------------------------------------------------------------------

__device__ __constant__ static const uint32_t BLAKE3_IV[8] = {
    0x6A09E667u, 0xBB67AE85u, 0x3C6EF372u, 0xA54FF53Au,
    0x510E527Fu, 0x9B05688Cu, 0x1F83D9ABu, 0x5BE0CD19u};

#define FLAG_CHUNK_START 1u
#define FLAG_CHUNK_END 2u
#define FLAG_ROOT 8u
#define FLAG_KEYED_HASH 16u

__device__ __forceinline__ uint32_t rotr32(uint32_t x, uint32_t n) {
  return (x >> n) | (x << (32u - n));
}

__device__ __forceinline__ void blake3_g(uint32_t *v, int a, int b, int c, int d,
                                         uint32_t mx, uint32_t my) {
  v[a] = v[a] + v[b] + mx;
  v[d] = rotr32(v[d] ^ v[a], 16);
  v[c] = v[c] + v[d];
  v[b] = rotr32(v[b] ^ v[c], 12);
  v[a] = v[a] + v[b] + my;
  v[d] = rotr32(v[d] ^ v[a], 8);
  v[c] = v[c] + v[d];
  v[b] = rotr32(v[b] ^ v[c], 7);
}

// The full 16-word state. Words 0..7 are the chaining value, or the first 32
// bytes of a root output; words 8..15 are the next 32 bytes of root output.
__device__ void blake3_compress(const uint32_t cv[8], const uint32_t block[16],
                                uint64_t counter, uint32_t block_len,
                                uint32_t flags, uint32_t out[16]) {
  uint32_t v[16];
#pragma unroll
  for (int i = 0; i < 8; ++i) {
    v[i] = cv[i];
    v[i + 8] = BLAKE3_IV[i];
  }
  v[12] = (uint32_t)counter;
  v[13] = (uint32_t)(counter >> 32);
  v[14] = block_len;
  v[15] = flags;

  uint32_t m[16];
#pragma unroll
  for (int i = 0; i < 16; ++i) {
    m[i] = block[i];
  }

  static const int PERMUTATION[16] = {2, 6, 3, 10, 7, 0, 4, 13,
                                      1, 11, 12, 5, 9, 14, 15, 8};

  for (int round = 0; round < 7; ++round) {
    blake3_g(v, 0, 4, 8, 12, m[0], m[1]);
    blake3_g(v, 1, 5, 9, 13, m[2], m[3]);
    blake3_g(v, 2, 6, 10, 14, m[4], m[5]);
    blake3_g(v, 3, 7, 11, 15, m[6], m[7]);
    blake3_g(v, 0, 5, 10, 15, m[8], m[9]);
    blake3_g(v, 1, 6, 11, 12, m[10], m[11]);
    blake3_g(v, 2, 7, 8, 13, m[12], m[13]);
    blake3_g(v, 3, 4, 9, 14, m[14], m[15]);

    // Not applied after the final round, exactly as in blake3_ref.rs.
    if (round < 6) {
      uint32_t permuted[16];
#pragma unroll
      for (int i = 0; i < 16; ++i) {
        permuted[i] = m[PERMUTATION[i]];
      }
#pragma unroll
      for (int i = 0; i < 16; ++i) {
        m[i] = permuted[i];
      }
    }
  }

#pragma unroll
  for (int i = 0; i < 8; ++i) {
    out[i] = v[i] ^ v[i + 8];
    out[i + 8] = v[i + 8] ^ cv[i];
  }
}

// Keyed BLAKE3 of one block, 64 bytes of output as sixteen words.
//
// `block` must already be zero-padded past `block_len`; the callers below build
// their blocks word by word and zero the tail explicitly, which is the same
// discipline `block_words` enforces on the Rust side.
__device__ __forceinline__ void keyed_512(const uint32_t key[8],
                                          const uint32_t block[16],
                                          uint32_t block_len,
                                          uint32_t out[16]) {
  blake3_compress(key, block, 0, block_len,
                  FLAG_KEYED_HASH | FLAG_CHUNK_START | FLAG_CHUNK_END |
                      FLAG_ROOT,
                  out);
}

// Keyed BLAKE3 of exactly two blocks, 32 bytes of output as eight words.
// The only two-block input in the protocol is hashimoto's 96-byte squeeze.
__device__ void keyed_256_two_blocks(const uint32_t key[8],
                                     const uint32_t first[16],
                                     const uint32_t second[16],
                                     uint32_t second_len, uint32_t out[8]) {
  uint32_t state[16];
  blake3_compress(key, first, 0, 64, FLAG_KEYED_HASH | FLAG_CHUNK_START, state);

  uint32_t chaining[8];
#pragma unroll
  for (int i = 0; i < 8; ++i) {
    chaining[i] = state[i];
  }

  blake3_compress(chaining, second, 0, second_len,
                  FLAG_KEYED_HASH | FLAG_CHUNK_END | FLAG_ROOT, state);
#pragma unroll
  for (int i = 0; i < 8; ++i) {
    out[i] = state[i];
  }
}

// ---------------------------------------------------------------------------
// the DAG
// ---------------------------------------------------------------------------

__device__ __forceinline__ uint32_t fnv(uint32_t a, uint32_t b) {
  return (a * FNV_PRIME) ^ b;
}

// One dataset item from the cache. The port of src/dag.rs::dataset_item.
__device__ void dataset_item(const uint32_t *__restrict__ cache,
                             uint32_t cache_items, uint32_t index,
                             const uint32_t item_key[8], uint32_t out[16]) {
  uint32_t mix[16];
  const uint32_t *seed = cache + (size_t)(index % cache_items) * ITEM_WORDS;
#pragma unroll
  for (int i = 0; i < ITEM_WORDS; ++i) {
    mix[i] = seed[i];
  }
  mix[0] ^= index;

  uint32_t hashed[16];
  keyed_512(item_key, mix, 64, hashed);
#pragma unroll
  for (int i = 0; i < ITEM_WORDS; ++i) {
    mix[i] = hashed[i];
  }

  for (uint32_t parent = 0; parent < DATASET_PARENTS; ++parent) {
    uint32_t selector =
        fnv(index ^ parent, mix[parent % ITEM_WORDS]) % cache_items;
    const uint32_t *item = cache + (size_t)selector * ITEM_WORDS;
#pragma unroll
    for (int i = 0; i < ITEM_WORDS; ++i) {
      mix[i] = fnv(mix[i], item[i]);
    }
  }

  keyed_512(item_key, mix, 64, hashed);
#pragma unroll
  for (int i = 0; i < ITEM_WORDS; ++i) {
    out[i] = hashed[i];
  }
}

// Materialises a slice of the dataset. Items are mutually independent given the
// cache, so this is embarrassingly parallel and needs no synchronisation.
extern "C" __global__ void maya_dag_generate_kernel(
    const uint32_t *__restrict__ cache, uint32_t cache_items,
    uint32_t *__restrict__ dataset, uint32_t first, uint32_t count,
    uint32_t k0, uint32_t k1, uint32_t k2, uint32_t k3, uint32_t k4,
    uint32_t k5, uint32_t k6, uint32_t k7) {
  uint32_t offset = blockIdx.x * blockDim.x + threadIdx.x;
  if (offset >= count) {
    return;
  }

  const uint32_t item_key[8] = {k0, k1, k2, k3, k4, k5, k6, k7};
  uint32_t index = first + offset;

  uint32_t item[16];
  dataset_item(cache, cache_items, index, item_key, item);

  uint32_t *out = dataset + (size_t)index * ITEM_WORDS;
#pragma unroll
  for (int i = 0; i < ITEM_WORDS; ++i) {
    out[i] = item[i];
  }
}

// Big-endian comparison: true when `digest` is at or below `target`.
// Byte order matters — the target is a 256-bit big-endian threshold, so the
// first differing byte decides, exactly as `meets_target` does on the host.
__device__ __forceinline__ bool meets_target(const uint32_t digest[8],
                                             const uint8_t target[32]) {
#pragma unroll
  for (int i = 0; i < 32; ++i) {
    uint8_t byte = (uint8_t)(digest[i >> 2] >> ((i & 3) * 8));
    if (byte != target[i]) {
      return byte < target[i];
    }
  }
  return true;
}

// One nonce per thread. The reads are dependent, so a thread is latency-bound
// by construction; throughput comes from having thousands of them in flight.
extern "C" __global__ void maya_hashimoto_search_kernel(
    const uint32_t *__restrict__ dataset, uint32_t pages,
    uint32_t s0, uint32_t s1, uint32_t s2, uint32_t s3, uint32_t s4,
    uint32_t s5, uint32_t s6, uint32_t s7, uint64_t start_nonce,
    uint64_t nonce_count, const uint8_t *__restrict__ target,
    uint32_t m0, uint32_t m1, uint32_t m2, uint32_t m3, uint32_t m4,
    uint32_t m5, uint32_t m6, uint32_t m7,
    unsigned long long *__restrict__ found_nonce,
    int *__restrict__ found_flag) {
  uint64_t lane = (uint64_t)blockIdx.x * blockDim.x + threadIdx.x;
  if (lane >= nonce_count) {
    return;
  }

  // Another thread already won this launch. Checked once, not per access: it is
  // an optimisation, not a correctness condition.
  if (*found_flag != 0) {
    return;
  }

  const uint32_t mix_key[8] = {m0, m1, m2, m3, m4, m5, m6, m7};
  uint64_t nonce = start_nonce + lane;

  // Seed block: 32 bytes of header seed then the nonce, little-endian, zero
  // padded to 64. block_len is 40 — the true length, not the padded one.
  uint32_t seed_block[16];
  seed_block[0] = s0;
  seed_block[1] = s1;
  seed_block[2] = s2;
  seed_block[3] = s3;
  seed_block[4] = s4;
  seed_block[5] = s5;
  seed_block[6] = s6;
  seed_block[7] = s7;
  seed_block[8] = (uint32_t)nonce;
  seed_block[9] = (uint32_t)(nonce >> 32);
#pragma unroll
  for (int i = 10; i < 16; ++i) {
    seed_block[i] = 0;
  }

  uint32_t seed_words[16];
  keyed_512(mix_key, seed_block, 40, seed_words);

  uint32_t mix[MIX_WORDS];
#pragma unroll
  for (int i = 0; i < MIX_WORDS; ++i) {
    mix[i] = seed_words[i % ITEM_WORDS];
  }

  for (uint32_t access = 0; access < ACCESSES; ++access) {
    uint32_t page = fnv(access ^ seed_words[0], mix[access % MIX_WORDS]) % pages;

    // Eight 16-byte loads: one fully-utilised 128-byte page.
    const uint4 *chunk = (const uint4 *)(dataset + (size_t)page * MIX_WORDS);
#pragma unroll
    for (int i = 0; i < 8; ++i) {
      uint4 quad = chunk[i];
      mix[i * 4 + 0] = fnv(mix[i * 4 + 0], quad.x);
      mix[i * 4 + 1] = fnv(mix[i * 4 + 1], quad.y);
      mix[i * 4 + 2] = fnv(mix[i * 4 + 2], quad.z);
      mix[i * 4 + 3] = fnv(mix[i * 4 + 3], quad.w);
    }
  }

  // Fold 32 words to 8, then squeeze seed || mix.
  uint32_t squeeze[16];
#pragma unroll
  for (int i = 0; i < 8; ++i) {
    squeeze[i] = fnv(fnv(fnv(mix[i * 4], mix[i * 4 + 1]), mix[i * 4 + 2]),
                     mix[i * 4 + 3]);
  }
#pragma unroll
  for (int i = 8; i < 16; ++i) {
    squeeze[i] = 0;
  }

  uint32_t digest[8];
  keyed_256_two_blocks(mix_key, seed_words, squeeze, 32, digest);

  if (meets_target(digest, target)) {
    // First writer wins. `atomicCAS` rather than a plain store so that two
    // solutions in one launch cannot interleave into a third, invalid nonce.
    if (atomicCAS(found_flag, 0, 1) == 0) {
      *found_nonce = (unsigned long long)nonce;
    }
  }
}

// ---------------------------------------------------------------------------
// host shim
// ---------------------------------------------------------------------------
//
// The Rust side calls these three functions and nothing else. Keeping the CUDA
// API entirely inside C++ means the FFI surface in `src/gpu.rs` is three plain
// functions with no pointer arithmetic and no allocator to get wrong.

struct MayaDagContext {
  uint32_t *cache;
  uint32_t *dataset;
  uint8_t *target;
  unsigned long long *found_nonce;
  int *found_flag;
  uint32_t cache_items;
  uint32_t dataset_pages;
};

#define MAYA_OK 0
#define MAYA_ERR_ALLOC -1
#define MAYA_ERR_COPY -2
#define MAYA_ERR_LAUNCH -3
#define MAYA_ERR_ARGUMENT -4

// Dataset generation is chunked so no single launch runs long enough to trip a
// display driver's watchdog on a card that is also driving a monitor.
#define GENERATE_CHUNK_ITEMS (1u << 20)
#define THREADS_PER_BLOCK 256u

// Declared ahead of `maya_dag_create`, which calls it on every failure path so
// that a partially built context never leaks device memory.
extern "C" void maya_dag_destroy(MayaDagContext *ctx);

extern "C" int maya_dag_create(MayaDagContext **out, int device,
                               const uint32_t *cache_words, uint32_t cache_items,
                               uint32_t dataset_pages, const uint32_t *item_key,
                               const uint32_t *mix_key) {
  (void)mix_key;
  if (out == nullptr || cache_words == nullptr || cache_items == 0 ||
      dataset_pages == 0) {
    return MAYA_ERR_ARGUMENT;
  }
  if (cudaSetDevice(device) != cudaSuccess) {
    return MAYA_ERR_ARGUMENT;
  }

  MayaDagContext *ctx = new MayaDagContext();
  ctx->cache = nullptr;
  ctx->dataset = nullptr;
  ctx->target = nullptr;
  ctx->found_nonce = nullptr;
  ctx->found_flag = nullptr;
  ctx->cache_items = cache_items;
  ctx->dataset_pages = dataset_pages;

  size_t cache_bytes = (size_t)cache_items * ITEM_WORDS * sizeof(uint32_t);
  size_t dataset_items = (size_t)dataset_pages * 2;
  size_t dataset_bytes = dataset_items * ITEM_WORDS * sizeof(uint32_t);

  if (cudaMalloc(&ctx->cache, cache_bytes) != cudaSuccess ||
      cudaMalloc(&ctx->dataset, dataset_bytes) != cudaSuccess ||
      cudaMalloc(&ctx->target, 32) != cudaSuccess ||
      cudaMalloc(&ctx->found_nonce, sizeof(unsigned long long)) != cudaSuccess ||
      cudaMalloc(&ctx->found_flag, sizeof(int)) != cudaSuccess) {
    maya_dag_destroy(ctx);
    return MAYA_ERR_ALLOC;
  }

  if (cudaMemcpy(ctx->cache, cache_words, cache_bytes,
                 cudaMemcpyHostToDevice) != cudaSuccess) {
    maya_dag_destroy(ctx);
    return MAYA_ERR_COPY;
  }

  // The dataset is built on the device from the uploaded cache. Never
  // transferred: 4 GiB over PCIe takes longer than generating it does, and the
  // host would need to hold a second copy to send.
  for (uint32_t first = 0; first < (uint32_t)dataset_items;
       first += GENERATE_CHUNK_ITEMS) {
    uint32_t count = GENERATE_CHUNK_ITEMS;
    if (first + count > (uint32_t)dataset_items) {
      count = (uint32_t)dataset_items - first;
    }
    uint32_t blocks = (count + THREADS_PER_BLOCK - 1) / THREADS_PER_BLOCK;

    maya_dag_generate_kernel<<<blocks, THREADS_PER_BLOCK>>>(
        ctx->cache, cache_items, ctx->dataset, first, count, item_key[0],
        item_key[1], item_key[2], item_key[3], item_key[4], item_key[5],
        item_key[6], item_key[7]);

    if (cudaDeviceSynchronize() != cudaSuccess) {
      maya_dag_destroy(ctx);
      return MAYA_ERR_LAUNCH;
    }
  }

  *out = ctx;
  return MAYA_OK;
}

// Searches `nonce_count` nonces from `start_nonce`. Returns MAYA_OK whether or
// not a solution was found; `found` says which.
extern "C" int maya_dag_search(MayaDagContext *ctx, const uint32_t *seed_words,
                               const uint32_t *mix_key, uint64_t start_nonce,
                               uint64_t nonce_count, const uint8_t *target,
                               uint64_t *found_nonce, int *found) {
  if (ctx == nullptr || seed_words == nullptr || target == nullptr ||
      found_nonce == nullptr || found == nullptr || nonce_count == 0) {
    return MAYA_ERR_ARGUMENT;
  }

  int zero = 0;
  if (cudaMemcpy(ctx->target, target, 32, cudaMemcpyHostToDevice) !=
          cudaSuccess ||
      cudaMemcpy(ctx->found_flag, &zero, sizeof(int), cudaMemcpyHostToDevice) !=
          cudaSuccess) {
    return MAYA_ERR_COPY;
  }

  uint64_t blocks = (nonce_count + THREADS_PER_BLOCK - 1) / THREADS_PER_BLOCK;
  maya_hashimoto_search_kernel<<<(unsigned int)blocks, THREADS_PER_BLOCK>>>(
      ctx->dataset, ctx->dataset_pages, seed_words[0], seed_words[1],
      seed_words[2], seed_words[3], seed_words[4], seed_words[5], seed_words[6],
      seed_words[7], start_nonce, nonce_count, ctx->target, mix_key[0],
      mix_key[1], mix_key[2], mix_key[3], mix_key[4], mix_key[5], mix_key[6],
      mix_key[7], ctx->found_nonce, ctx->found_flag);

  if (cudaDeviceSynchronize() != cudaSuccess) {
    return MAYA_ERR_LAUNCH;
  }

  int flag = 0;
  unsigned long long nonce = 0;
  if (cudaMemcpy(&flag, ctx->found_flag, sizeof(int), cudaMemcpyDeviceToHost) !=
          cudaSuccess ||
      cudaMemcpy(&nonce, ctx->found_nonce, sizeof(unsigned long long),
                 cudaMemcpyDeviceToHost) != cudaSuccess) {
    return MAYA_ERR_COPY;
  }

  *found = flag;
  *found_nonce = (uint64_t)nonce;
  return MAYA_OK;
}

// Copies dataset items back to the host, for the parity test that compares a
// GPU-generated dataset against the CPU reference. Never used by the miner
// itself — the dataset exists to be read on the device.
extern "C" int maya_dag_read_items(MayaDagContext *ctx, uint32_t first,
                                   uint32_t count, uint32_t *out) {
  if (ctx == nullptr || out == nullptr) {
    return MAYA_ERR_ARGUMENT;
  }
  size_t offset = (size_t)first * ITEM_WORDS * sizeof(uint32_t);
  size_t bytes = (size_t)count * ITEM_WORDS * sizeof(uint32_t);
  if (cudaMemcpy(out, (const uint8_t *)ctx->dataset + offset, bytes,
                 cudaMemcpyDeviceToHost) != cudaSuccess) {
    return MAYA_ERR_COPY;
  }
  return MAYA_OK;
}

extern "C" void maya_dag_destroy(MayaDagContext *ctx) {
  if (ctx == nullptr) {
    return;
  }
  if (ctx->cache != nullptr) {
    cudaFree(ctx->cache);
  }
  if (ctx->dataset != nullptr) {
    cudaFree(ctx->dataset);
  }
  if (ctx->target != nullptr) {
    cudaFree(ctx->target);
  }
  if (ctx->found_nonce != nullptr) {
    cudaFree(ctx->found_nonce);
  }
  if (ctx->found_flag != nullptr) {
    cudaFree(ctx->found_flag);
  }
  delete ctx;
}

extern "C" int maya_dag_device_count(int *count) {
  if (count == nullptr) {
    return MAYA_ERR_ARGUMENT;
  }
  if (cudaGetDeviceCount(count) != cudaSuccess) {
    *count = 0;
  }
  return MAYA_OK;
}
