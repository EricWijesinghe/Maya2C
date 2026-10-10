//! The DAG proof of work, as the GPU miner computes it.
//!
//! ## Why the miner has its own copy
//!
//! This crate does not depend on the node — deliberately, so that shipping a
//! miner does not drag `RocksDB` and libp2p along with it — so the definition it
//! mines against is written out here. That is a duplication, and duplication of
//! a consensus rule is exactly the thing that produces silently rejected
//! blocks, so it is checked from two directions:
//!
//! * `tests/dag_parity.rs` runs this against the node's implementation on the
//!   same inputs, and against the frozen vectors in `tests/fixtures/`.
//! * Everything hashes through [`crate::blake3_ref`], which is itself pinned to
//!   the `blake3` crate.
//!
//! ## Why it is written the way the kernel needs it
//!
//! [`dataset_item`] takes a flat `&[u32]` cache rather than a slice of items,
//! indexes it arithmetically, and keeps its whole working set in sixteen
//! registers. That is not idiomatic Rust; it is what the CUDA transliteration
//! in `kernels/dag.cu` needs to be a transliteration rather than a rewrite. The
//! node's version is free to be idiomatic because nothing has to be ported
//! from it.

use crate::blake3_ref::{keyed_256, keyed_512};

/// Bytes in one cache or dataset item.
pub const ITEM_BYTES: usize = 64;

/// Words in one item.
pub const ITEM_WORDS: usize = ITEM_BYTES / 4;

/// Bytes read per dataset lookup: two adjacent items.
pub const MIX_BYTES: usize = 128;

/// Words in one mix.
pub const MIX_WORDS: usize = MIX_BYTES / 4;

/// Cache items mixed into each dataset item.
pub const DATASET_PARENTS: u32 = 256;

/// Passes of the sequential memory-hard cache construction.
pub const CACHE_ROUNDS: usize = 3;

/// Dataset pages read per hash.
pub const ACCESSES: u32 = 64;

/// The FNV-1a prime, as used by Ethash's mixing function.
pub const FNV_PRIME: u32 = 0x0100_0193;

/// Bytes in a header, and in the seed a search runs against.
pub const SEED_BYTES: usize = 32;

// Domain-separation contexts. Byte-identical to `src/crypto/dag/mod.rs` and
// `src/core/block.rs` in the node; a single changed character here produces a
// miner that computes a different chain's proof of work and never lands a
// block. `contexts_match_the_node` in the parity tests asserts the derived keys
// rather than trusting the strings.
const SEED_CONTEXT: &str = "custom-l1-node 2026-08-31 dag epoch seed v1";
const CACHE_CONTEXT: &str = "custom-l1-node 2026-08-31 dag cache item v1";
const ITEM_CONTEXT: &str = "custom-l1-node 2026-08-31 dag dataset item v1";
const MIX_CONTEXT: &str = "custom-l1-node 2026-08-31 dag hashimoto mix v1";
const POW_SEED_CONTEXT: &str = "custom-l1-node 2026-08-31 dag pow seed v1";

/// One 64-byte item, as sixteen little-endian words.
pub type Item = [u32; ITEM_WORDS];

/// An all-zero item.
pub const ZERO_ITEM: Item = [0u32; ITEM_WORDS];

/// The sizes one DAG instance is built at. Mirrors the node's `Params`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// Blocks between dataset regenerations.
    pub epoch_length: u64,
    /// Nominal cache size in bytes, before the prime adjustment.
    pub cache_bytes: usize,
    /// Nominal dataset size in bytes, before the prime adjustment.
    pub dataset_bytes: usize,
}

impl Params {
    /// Consensus sizes: 64 MiB cache, 4 GiB dataset.
    pub const MAINNET: Self = Self {
        epoch_length: 30_000,
        cache_bytes: 64 * 1024 * 1024,
        dataset_bytes: 4 * 1024 * 1024 * 1024,
    };

    /// Test sizes: 512 KiB cache, 4 MiB dataset.
    pub const TESTING: Self = Self {
        epoch_length: 8,
        cache_bytes: 512 * 1024,
        dataset_bytes: 4 * 1024 * 1024,
    };

    /// Items in the cache. Prime, for the reason the node gives.
    #[must_use]
    pub fn cache_items(&self) -> u32 {
        largest_prime_at_most(u32::try_from(self.cache_bytes / ITEM_BYTES).unwrap_or(u32::MAX))
    }

    /// Pages in the dataset. Prime.
    #[must_use]
    pub fn dataset_pages(&self) -> u32 {
        largest_prime_at_most(u32::try_from(self.dataset_bytes / MIX_BYTES).unwrap_or(u32::MAX))
    }

    /// Items in the dataset. Two per page.
    #[must_use]
    pub fn dataset_items(&self) -> u32 {
        self.dataset_pages() * 2
    }

    /// The epoch a block at `height` belongs to.
    #[must_use]
    pub fn epoch_of(&self, height: u64) -> u64 {
        height / self.epoch_length.max(1)
    }
}

/// The largest prime not exceeding `n`.
#[must_use]
pub fn largest_prime_at_most(n: u32) -> u32 {
    let mut candidate = if n.is_multiple_of(2) { n - 1 } else { n };
    while candidate >= 3 {
        if is_prime(candidate) {
            return candidate;
        }
        candidate -= 2;
    }
    2
}

/// Trial-division primality test.
#[must_use]
pub fn is_prime(n: u32) -> bool {
    if n < 2 {
        return false;
    }
    if n < 4 {
        return true;
    }
    if n.is_multiple_of(2) {
        return false;
    }

    let mut divisor = 3u32;
    while divisor.saturating_mul(divisor) <= n {
        if n.is_multiple_of(divisor) {
            return false;
        }
        divisor += 2;
    }
    true
}

/// The four domain keys, derived once and handed to the kernel as bytes.
///
/// Key derivation is a two-pass BLAKE3 over a context string, and it happens
/// four times per process. Porting it to CUDA would be more device code to keep
/// in step for no gain, so the device is given the finished keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keys {
    /// Key for cache-item hashing.
    pub cache: [u8; 32],
    /// Key for dataset-item hashing.
    pub item: [u8; 32],
    /// Key for the hashimoto mix and its final squeeze.
    pub mix: [u8; 32],
}

impl Keys {
    /// Derives the keys. Cheap, and stable for the life of the protocol.
    #[must_use]
    pub fn derive() -> Self {
        Self {
            cache: blake3::derive_key(CACHE_CONTEXT, &[]),
            item: blake3::derive_key(ITEM_CONTEXT, &[]),
            mix: blake3::derive_key(MIX_CONTEXT, &[]),
        }
    }
}

impl Default for Keys {
    fn default() -> Self {
        Self::derive()
    }
}

/// The seed for `epoch`, chained from the genesis seed.
#[must_use]
pub fn epoch_seed(epoch: u64) -> [u8; 32] {
    let mut seed = blake3::derive_key(SEED_CONTEXT, &[]);
    for _ in 0..epoch {
        seed = blake3::derive_key(SEED_CONTEXT, &seed);
    }
    seed
}

/// The 32 bytes a search runs against: the header with its nonce zeroed.
///
/// The header layout is the node's: 144 bytes with the nonce at 72..80. A miner
/// receiving raw header bytes from a pool calls this once per job.
///
/// # Panics
///
/// Panics if `header` is shorter than the nonce field — a malformed job, which
/// a caller should have rejected before reaching the hash.
#[must_use]
pub fn pow_seed(header: &[u8]) -> [u8; 32] {
    let mut bytes = header.to_vec();
    bytes[72..80].fill(0);
    blake3::derive_key(POW_SEED_CONTEXT, &bytes)
}

/// Reinterprets 64 bytes as sixteen little-endian words.
#[must_use]
pub fn item_from_le_bytes(bytes: &[u8; ITEM_BYTES]) -> Item {
    let mut item = ZERO_ITEM;
    let (chunks, _) = bytes.as_chunks::<4>();
    for (slot, chunk) in item.iter_mut().zip(chunks) {
        *slot = u32::from_le_bytes(*chunk);
    }
    item
}

/// The inverse of [`item_from_le_bytes`].
#[must_use]
pub fn item_to_le_bytes(item: &Item) -> [u8; ITEM_BYTES] {
    let mut bytes = [0u8; ITEM_BYTES];
    let (chunks, _) = bytes.as_chunks_mut::<4>();
    for (word, chunk) in item.iter().zip(chunks) {
        *chunk = word.to_le_bytes();
    }
    bytes
}

/// Ethash's mixing step: `(a × FNV_PRIME) ⊕ b`, modulo 2³².
#[inline]
#[must_use]
pub fn fnv(a: u32, b: u32) -> u32 {
    a.wrapping_mul(FNV_PRIME) ^ b
}

/// Generates the verification cache for `epoch`, as a flat word array.
///
/// Flat rather than `Vec<Item>` because this is what gets uploaded to the
/// device: a contiguous `u32` buffer the kernel indexes arithmetically.
#[must_use]
pub fn generate_cache(epoch: u64, params: Params, keys: &Keys) -> Vec<u32> {
    let count = params.cache_items() as usize;
    let mut cache = vec![0u32; count * ITEM_WORDS];

    // Pass one: a plain hash chain seeded by the epoch.
    let mut previous = keyed_512(&keys.cache, &epoch_seed(epoch));
    write_item(&mut cache, 0, &item_from_le_bytes(&previous));
    for index in 1..count {
        previous = keyed_512(&keys.cache, &previous);
        write_item(&mut cache, index, &item_from_le_bytes(&previous));
    }

    // Passes two and after: data-dependent rewrites.
    for _ in 0..CACHE_ROUNDS {
        for index in 0..count {
            let previous_index = if index == 0 { count - 1 } else { index - 1 };
            let scrambled = cache[index * ITEM_WORDS] as usize % count;

            let mut mixed = ZERO_ITEM;
            for (word, slot) in mixed.iter_mut().enumerate() {
                *slot = cache[previous_index * ITEM_WORDS + word]
                    ^ cache[scrambled * ITEM_WORDS + word];
            }

            let hashed = keyed_512(&keys.cache, &item_to_le_bytes(&mixed));
            write_item(&mut cache, index, &item_from_le_bytes(&hashed));
        }
    }

    cache
}

fn write_item(cache: &mut [u32], index: usize, item: &Item) {
    cache[index * ITEM_WORDS..(index + 1) * ITEM_WORDS].copy_from_slice(item);
}

/// Computes dataset item `index` from a flat cache.
///
/// The function `kernels/dag.cu` implements one thread at a time. Sixteen words
/// of state, 256 dependent reads into the cache, two hashes.
#[must_use]
pub fn dataset_item(cache: &[u32], index: u32, keys: &Keys) -> Item {
    let count = (cache.len() / ITEM_WORDS) as u32;

    let seed_index = (index % count) as usize * ITEM_WORDS;
    let mut mix: Item = ZERO_ITEM;
    mix.copy_from_slice(&cache[seed_index..seed_index + ITEM_WORDS]);
    mix[0] ^= index;
    mix = item_from_le_bytes(&keyed_512(&keys.item, &item_to_le_bytes(&mix)));

    for parent in 0..DATASET_PARENTS {
        let selector = fnv(index ^ parent, mix[parent as usize % ITEM_WORDS]) % count;
        let base = selector as usize * ITEM_WORDS;
        for (word, slot) in mix.iter_mut().enumerate() {
            *slot = fnv(*slot, cache[base + word]);
        }
    }

    item_from_le_bytes(&keyed_512(&keys.item, &item_to_le_bytes(&mix)))
}

/// The output of one hashimoto evaluation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proof {
    /// The compressed mix.
    pub mix: [u8; 32],
    /// The digest compared against the difficulty target.
    pub result: [u8; 32],
}

/// Evaluates hashimoto against a materialised dataset. The miner's path.
#[must_use]
pub fn hashimoto_full(
    dataset: &[u32],
    pages: u32,
    seed: &[u8; 32],
    nonce: u64,
    keys: &Keys,
) -> Proof {
    hashimoto(pages, seed, nonce, keys, |page| {
        let base = page as usize * MIX_WORDS;
        let mut words = [0u32; MIX_WORDS];
        words.copy_from_slice(&dataset[base..base + MIX_WORDS]);
        words
    })
}

/// Evaluates hashimoto by recomputing pages from the cache. The validator's
/// path, kept here so the miner can check its own solution before submitting.
#[must_use]
pub fn hashimoto_light(
    cache: &[u32],
    pages: u32,
    seed: &[u8; 32],
    nonce: u64,
    keys: &Keys,
) -> Proof {
    hashimoto(pages, seed, nonce, keys, |page| {
        let first = dataset_item(cache, page * 2, keys);
        let second = dataset_item(cache, page * 2 + 1, keys);

        let mut words = [0u32; MIX_WORDS];
        words[..ITEM_WORDS].copy_from_slice(&first);
        words[ITEM_WORDS..].copy_from_slice(&second);
        words
    })
}

/// The shared body; `lookup` is the only difference between the two paths.
fn hashimoto(
    pages: u32,
    seed: &[u8; 32],
    nonce: u64,
    keys: &Keys,
    lookup: impl Fn(u32) -> [u32; MIX_WORDS],
) -> Proof {
    let mut seed_input = [0u8; SEED_BYTES + 8];
    seed_input[..SEED_BYTES].copy_from_slice(seed);
    seed_input[SEED_BYTES..].copy_from_slice(&nonce.to_le_bytes());
    let seed_bytes = keyed_512(&keys.mix, &seed_input);
    let seed_words = item_from_le_bytes(&seed_bytes);

    let mut mix = [0u32; MIX_WORDS];
    for (index, slot) in mix.iter_mut().enumerate() {
        *slot = seed_words[index % ITEM_WORDS];
    }

    for access in 0..ACCESSES {
        let page = fnv(access ^ seed_words[0], mix[access as usize % MIX_WORDS]) % pages;
        let page_words = lookup(page);
        for (slot, word) in mix.iter_mut().zip(page_words.iter()) {
            *slot = fnv(*slot, *word);
        }
    }

    let mut mix_bytes = [0u8; 32];
    for index in 0..MIX_WORDS / 4 {
        let base = index * 4;
        let folded = fnv(
            fnv(fnv(mix[base], mix[base + 1]), mix[base + 2]),
            mix[base + 3],
        );
        mix_bytes[index * 4..index * 4 + 4].copy_from_slice(&folded.to_le_bytes());
    }

    let mut squeeze = [0u8; 64 + 32];
    squeeze[..64].copy_from_slice(&seed_bytes);
    squeeze[64..].copy_from_slice(&mix_bytes);

    Proof {
        mix: mix_bytes,
        result: keyed_256(&keys.mix, &squeeze),
    }
}

/// Generates the full dataset on the CPU, for tests and for a machine with no
/// GPU.
///
/// The real miner generates this on the device from the uploaded cache — a
/// 4 GiB `PCIe` transfer would take longer than the generation does. This exists
/// so the kernel has something to be compared against.
#[must_use]
pub fn generate_dataset(cache: &[u32], params: Params, keys: &Keys) -> Vec<u32> {
    let count = params.dataset_items() as usize;
    let mut dataset = vec![0u32; count * ITEM_WORDS];
    for index in 0..count {
        let item = dataset_item(cache, index as u32, keys);
        write_item(&mut dataset, index, &item);
    }
    dataset
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const TEST: Params = Params::TESTING;

    #[test]
    fn the_prime_counts_land_where_the_node_puts_them() {
        assert!(is_prime(TEST.cache_items()));
        assert!(is_prime(TEST.dataset_pages()));
        assert!(is_prime(Params::MAINNET.cache_items()));
        assert!(is_prime(Params::MAINNET.dataset_pages()));
        assert_eq!(largest_prime_at_most(100), 97);
    }

    #[test]
    fn items_round_trip_through_their_byte_form() {
        let mut item = ZERO_ITEM;
        for (index, word) in item.iter_mut().enumerate() {
            *word = (index as u32).wrapping_mul(0x9E37_79B9);
        }
        assert_eq!(item_from_le_bytes(&item_to_le_bytes(&item)), item);
    }

    #[test]
    fn light_and_full_agree() {
        // Same property the node asserts, in the miner's own implementation:
        // what the GPU searches with and what the miner self-checks with must
        // be the same function.
        let keys = Keys::derive();
        let cache = generate_cache(0, TEST, &keys);
        let dataset = generate_dataset(&cache, TEST, &keys);
        let pages = TEST.dataset_pages();
        let seed = [0xA5u8; 32];

        for nonce in 0..8u64 {
            assert_eq!(
                hashimoto_light(&cache, pages, &seed, nonce, &keys),
                hashimoto_full(&dataset, pages, &seed, nonce, &keys),
                "nonce {nonce} diverged"
            );
        }
    }

    #[test]
    fn zeroing_the_nonce_is_what_makes_the_seed_constant() {
        let mut header = vec![0x11u8; crate::HEADER_LEN];
        header[72..80].copy_from_slice(&7u64.to_le_bytes());
        let with_seven = pow_seed(&header);

        header[72..80].copy_from_slice(&9u64.to_le_bytes());
        assert_eq!(
            pow_seed(&header),
            with_seven,
            "the nonce leaked into the seed"
        );

        header[0] ^= 1;
        assert_ne!(pow_seed(&header), with_seven, "the header did not reach it");
    }
}
