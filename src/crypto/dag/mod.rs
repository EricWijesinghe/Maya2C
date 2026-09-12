//! A memory-hard, DAG-based proof of work.
//!
//! Structurally this is Ethash: a small **cache** that every node holds, and a
//! large **dataset** derived from it that only miners hold. A hash reads
//! pseudo-random pages of the dataset, so the work is bound by memory bandwidth
//! rather than by arithmetic. Verification recomputes the handful of pages it
//! needs from the cache, which is what keeps a validator's footprint at tens of
//! megabytes instead of gigabytes.
//!
//! ## What this is for, stated honestly
//!
//! The goal is **not** to make general-purpose hardware beat purpose-built
//! silicon. Nothing has ever achieved that, and the design this one is modelled
//! on is the proof: Ethash was the canonical ASIC-resistant DAG, and Ethash
//! ASICs shipped in April 2018 at roughly 2–2.5× the efficiency of contemporary
//! GPUs. The mechanism is structural. When an algorithm is bound by commodity
//! DRAM bandwidth, an ASIC buys the *same* commodity DRAM and then deletes the
//! display engine, the shader array, the PCIe complex and the driver stack.
//!
//! What a DAG does buy is a **ceiling on that advantage**. A SHA-256 ASIC beats
//! a CPU by roughly four orders of magnitude because the work is pure
//! arithmetic and arithmetic is what custom silicon is best at. Here the work is
//! DRAM traffic, which custom silicon has to buy on the same open market as
//! everyone else. The realistic outcome is a 2–5× gap rather than a 10,000×
//! one — which is the difference between GPU mining being worthless and GPU
//! mining being viable. That is the claim this module makes, and it is the only
//! one it can support.
//!
//! ## Design target: GDDR6/6X/7, not HBM
//!
//! Deliberately tuned for 400–1000 GB/s commodity graphics memory. Tuning for
//! HBM3 would favour datacenter accelerators — the most centralized and least
//! commodity hardware there is — which is the opposite of the point.
//!
//! ## Fixed size, rotating contents
//!
//! Ethash grows its dataset every epoch, which is what eventually made 4 GB
//! cards unable to mine Ethereum at all. Here [`DATASET_BYTES`] is constant and
//! only the *contents* rotate, once per [`EPOCH_LENGTH`] blocks. A card that
//! can mine today can mine indefinitely, and the hardware floor is a published
//! constant rather than a moving deadline.
//!
//! ## Hashing primitive
//!
//! BLAKE3 throughout, where Ethash uses Keccak. It is already the chain's hash
//! (`src/core/block.rs:76`, `src/crypto/argon_blake.rs:84`), so this adds no
//! dependency and no second primitive to audit.
//!
//! Domain separation uses derived keys rather than `Hasher::new_derive_key` in
//! the hot loop: `new_derive_key` re-hashes the context *string* on every call,
//! and dataset generation makes on the order of 10⁸ of them. The context is
//! hashed once into a 32-byte key (`cache_key`, `item_key`, `mix_key`)
//! and keyed hashing is used thereafter, which is one compression per call.

pub mod cache;
pub mod dataset;
pub mod hashimoto;
pub mod params;
pub mod registry;

use std::sync::LazyLock;

pub use params::Params;

/// Bytes in one cache or dataset item.
pub const ITEM_BYTES: usize = 64;

/// Bytes read per dataset lookup: two adjacent items.
///
/// A 128-byte page is the smallest read that keeps a GPU's memory transactions
/// fully utilised — four 32-byte sectors, all of them wanted — while staying
/// small enough that the access pattern is still random rather than streaming.
pub const MIX_BYTES: usize = 128;

/// 32-bit words in one mix.
pub const MIX_WORDS: usize = MIX_BYTES / 4;

/// Blocks between dataset regenerations.
///
/// At `TARGET_BLOCK_TIME` of 15 seconds this is 450,000 seconds, or 5.21 days.
pub const EPOCH_LENGTH: u64 = 30_000;

/// First height whose proof of work is the DAG rather than ArgonBlake.
///
/// The switch is a hard fork: a block at or above this height is checked with
/// [`hashimoto`] against the epoch's cache, and one below it with
/// `argon_blake_hash`. Both rules stay in the binary permanently, because a
/// node syncing from genesis has to be able to check the pre-fork chain.
///
/// Placed on an epoch boundary — the start of epoch 1 — so that the first DAG
/// block is also the first block of a fresh dataset, and no epoch is ever half
/// ArgonBlake and half DAG. Miners get the whole of epoch 0, five days, to
/// build the dataset before it is worth anything.
pub const DAG_ACTIVATION_HEIGHT: u64 = EPOCH_LENGTH;

/// Nominal dataset size: 4 GiB.
///
/// The realised size is slightly under this — see [`dataset_pages`], which
/// rounds down to a prime page count.
pub const DATASET_BYTES: usize = 4 * 1024 * 1024 * 1024;

/// Nominal cache size: 64 MiB, 1/64th of the dataset.
///
/// This is what a validating node holds. It is the number that decides whether
/// running a full node stays cheap, so it is deliberately far below the
/// dataset: a validator recomputes the pages it needs and never allocates the
/// 4 GiB a miner does.
pub const CACHE_BYTES: usize = 64 * 1024 * 1024;

/// Cache items mixed into each dataset item.
pub const DATASET_PARENTS: u32 = 256;

/// Passes of the sequential memory-hard cache construction.
pub const CACHE_ROUNDS: usize = 3;

/// Dataset pages read per hash.
pub const ACCESSES: usize = 64;

/// The FNV-1a prime, as used by Ethash's mixing function.
pub const FNV_PRIME: u32 = 0x0100_0193;

// Domain-separation contexts. Distinct constants keep the three BLAKE3 uses
// from colliding even on identical input, exactly as
// `src/crypto/argon_blake.rs:60` does for its own three stages.
const SEED_CONTEXT: &str = "custom-l1-node 2026-08-31 dag epoch seed v1";
const CACHE_CONTEXT: &str = "custom-l1-node 2026-08-31 dag cache item v1";
const ITEM_CONTEXT: &str = "custom-l1-node 2026-08-31 dag dataset item v1";
const MIX_CONTEXT: &str = "custom-l1-node 2026-08-31 dag hashimoto mix v1";

/// Key for cache-item hashing. Derived once; see the module docs.
pub(crate) fn cache_key() -> &'static [u8; 32] {
    static KEY: LazyLock<[u8; 32]> = LazyLock::new(|| blake3::derive_key(CACHE_CONTEXT, &[]));
    &KEY
}

/// Key for dataset-item hashing.
///
/// Defined here rather than beside its consumer so that all four domain
/// separators sit together: someone auditing domain separation should be able
/// to read them in one place, and `the_three_domain_keys_are_distinct` needs
/// them together to be written at all.
pub(crate) fn item_key() -> &'static [u8; 32] {
    static KEY: LazyLock<[u8; 32]> = LazyLock::new(|| blake3::derive_key(ITEM_CONTEXT, &[]));
    &KEY
}

/// Key for the hashimoto mix. See [`item_key`] for why it lives here.
pub(crate) fn mix_key() -> &'static [u8; 32] {
    static KEY: LazyLock<[u8; 32]> = LazyLock::new(|| blake3::derive_key(MIX_CONTEXT, &[]));
    &KEY
}

/// One 64-byte cache or dataset item, addressed as sixteen little-endian words.
pub type Item = [u32; ITEM_BYTES / 4];

/// An all-zero item.
pub const ZERO_ITEM: Item = [0u32; ITEM_BYTES / 4];

/// The epoch a block at `height` belongs to.
#[must_use]
pub fn epoch_of(height: u64) -> u64 {
    height / EPOCH_LENGTH
}

/// The first height in `epoch`.
///
/// Returns `None` if the epoch is far enough out that its first height would
/// overflow — unreachable on any real chain, and an explicit `None` rather than
/// a wrapped height that would silently select the wrong dataset.
#[must_use]
pub fn epoch_start(epoch: u64) -> Option<u64> {
    epoch.checked_mul(EPOCH_LENGTH)
}

/// The seed for `epoch`, chained from the genesis seed.
///
/// `seed(0)` is a constant; `seed(n) = H(seed(n-1))`. Chaining rather than
/// hashing the epoch number directly means an implementation cannot arrive at a
/// later epoch's seed without having walked every earlier one, which makes the
/// sequence a single fixed line no shortcut can rejoin at the wrong point.
///
/// Cost is one BLAKE3 per epoch, and epochs pass at roughly seventy a year.
#[must_use]
pub fn epoch_seed(epoch: u64) -> [u8; 32] {
    let mut seed = blake3::derive_key(SEED_CONTEXT, &[]);
    for _ in 0..epoch {
        seed = blake3::derive_key(SEED_CONTEXT, &seed);
    }
    seed
}

/// Number of 64-byte items in the mainnet cache.
///
/// Rounded down to a prime. A composite count shares factors with the strides
/// the mixing function produces, which lets the walk fall into cycles far
/// shorter than the cache — the cache would then be nominally 64 MiB and
/// effectively a fraction of it, and the memory-hardness claim would be false
/// while every test still passed. Ethash does the same for the same reason.
///
/// This is [`Params::MAINNET`] memoized. Code that must work at any parameter
/// set — everything in [`dataset`], [`hashimoto`] and [`cache`] — takes the
/// count from its [`Params`] instead of calling this.
#[must_use]
pub fn cache_items() -> u32 {
    static ITEMS: LazyLock<u32> = LazyLock::new(|| Params::MAINNET.cache_items());
    *ITEMS
}

/// Number of 128-byte pages in the mainnet dataset.
///
/// Prime, for the reason given on [`cache_items`].
#[must_use]
pub fn dataset_pages() -> u32 {
    static PAGES: LazyLock<u32> = LazyLock::new(|| Params::MAINNET.dataset_pages());
    *PAGES
}

/// Number of 64-byte items in the dataset. Two per page.
#[must_use]
pub fn dataset_items() -> u32 {
    // Cannot overflow: `dataset_pages` is at most 2^25 for a 4 GiB dataset.
    dataset_pages() * 2
}

/// Realised cache size in bytes, after the prime adjustment.
#[must_use]
pub fn cache_bytes() -> usize {
    cache_items() as usize * ITEM_BYTES
}

/// Realised dataset size in bytes, after the prime adjustment.
#[must_use]
pub fn dataset_bytes() -> usize {
    dataset_pages() as usize * MIX_BYTES
}

/// Ethash's mixing step: `(a × FNV_PRIME) ⊕ b`, modulo 2³².
///
/// Not FNV-1a proper — the real construction hashes a byte at a time — but the
/// name is Ethash's and keeping it is what makes the correspondence legible.
/// Its job is to make each mix word depend on the whole page cheaply enough
/// that the memory read stays the bottleneck, which is the entire point.
#[inline]
#[must_use]
pub fn fnv(a: u32, b: u32) -> u32 {
    a.wrapping_mul(FNV_PRIME) ^ b
}

/// Applies [`fnv`] word-wise across two items.
#[inline]
#[must_use]
pub fn fnv_item(mix: &Item, data: &Item) -> Item {
    let mut out = ZERO_ITEM;
    for (slot, (a, b)) in out.iter_mut().zip(mix.iter().zip(data.iter())) {
        *slot = fnv(*a, *b);
    }
    out
}

/// Reinterprets 64 bytes as sixteen little-endian words.
#[must_use]
pub fn item_from_le_bytes(bytes: &[u8; ITEM_BYTES]) -> Item {
    let mut item = ZERO_ITEM;
    let (words, _) = bytes.as_chunks::<4>();
    for (slot, word) in item.iter_mut().zip(words) {
        *slot = u32::from_le_bytes(*word);
    }
    item
}

/// The inverse of [`item_from_le_bytes`].
#[must_use]
pub fn item_to_le_bytes(item: &Item) -> [u8; ITEM_BYTES] {
    let mut bytes = [0u8; ITEM_BYTES];
    let (words, _) = bytes.as_chunks_mut::<4>();
    for (word, slot) in item.iter().zip(words) {
        *slot = word.to_le_bytes();
    }
    bytes
}

/// A 64-byte keyed BLAKE3 digest.
///
/// BLAKE3's extendable output supplies the 64 bytes Ethash gets from
/// Keccak-512. Keyed rather than derive-key mode; see the module docs.
#[must_use]
pub fn blake3_512(key: &[u8; 32], data: &[u8]) -> [u8; ITEM_BYTES] {
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(data);
    let mut out = [0u8; ITEM_BYTES];
    hasher.finalize_xof().fill(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epochs_partition_the_height_space_at_the_stated_boundary() {
        assert_eq!(epoch_of(0), 0);
        assert_eq!(epoch_of(EPOCH_LENGTH - 1), 0);
        assert_eq!(epoch_of(EPOCH_LENGTH), 1);
        assert_eq!(epoch_of(EPOCH_LENGTH * 2 - 1), 1);
        assert_eq!(epoch_of(EPOCH_LENGTH * 2), 2);

        // The boundary is the thing a miner pre-generates against, so it is
        // asserted from both directions.
        for epoch in 0..5u64 {
            let start = epoch_start(epoch).expect("small epochs cannot overflow");
            assert_eq!(epoch_of(start), epoch);
            if start > 0 {
                assert_eq!(epoch_of(start - 1), epoch - 1);
            }
        }
    }

    #[test]
    fn an_epoch_start_that_would_overflow_is_none_rather_than_wrapping() {
        assert_eq!(epoch_start(u64::MAX), None);
        assert!(epoch_start(u64::MAX / EPOCH_LENGTH).is_some());
    }

    #[test]
    fn the_epoch_period_is_the_advertised_five_days() {
        // 30,000 blocks at the chain's 15-second target.
        let seconds = EPOCH_LENGTH * crate::consensus::difficulty::TARGET_BLOCK_TIME;
        assert_eq!(seconds, 450_000);
        let days = seconds as f64 / 86_400.0;
        assert!((days - 5.208).abs() < 0.01, "epoch is {days} days");
    }

    #[test]
    fn epoch_seeds_are_deterministic_and_all_distinct() {
        let seeds: Vec<[u8; 32]> = (0..8).map(epoch_seed).collect();

        for (i, seed) in seeds.iter().enumerate() {
            assert_eq!(*seed, epoch_seed(i as u64), "seed {i} is not deterministic");
            assert_ne!(*seed, [0u8; 32]);
        }
        for i in 0..seeds.len() {
            for j in (i + 1)..seeds.len() {
                assert_ne!(seeds[i], seeds[j], "epochs {i} and {j} share a seed");
            }
        }
    }

    #[test]
    fn the_seed_chain_advances_by_exactly_one_hash_per_epoch() {
        // The property a miner relies on to precompute the next epoch: seed(n+1)
        // is reachable from seed(n) alone.
        for epoch in 0..6u64 {
            let expected = blake3::derive_key(SEED_CONTEXT, &epoch_seed(epoch));
            assert_eq!(epoch_seed(epoch + 1), expected);
        }
    }

    #[test]
    fn cache_and_dataset_counts_are_prime() {
        assert!(
            params::is_prime(cache_items()),
            "cache item count is composite"
        );
        assert!(
            params::is_prime(dataset_pages()),
            "dataset page count is composite"
        );
    }

    #[test]
    fn the_memoized_mainnet_counts_match_the_parameter_set() {
        // The free functions are a memo over `Params::MAINNET`. If they ever
        // drifted from it, consensus and the generators would size differently.
        assert_eq!(cache_items(), Params::MAINNET.cache_items());
        assert_eq!(dataset_pages(), Params::MAINNET.dataset_pages());
        assert_eq!(cache_bytes(), Params::MAINNET.realised_cache_bytes());
        assert_eq!(dataset_bytes(), Params::MAINNET.realised_dataset_bytes());
        assert_eq!(epoch_of(12_345), Params::MAINNET.epoch_of(12_345));
    }

    #[test]
    fn the_fork_lands_on_an_epoch_boundary() {
        // A half-ArgonBlake, half-DAG epoch would mean the first DAG block sat
        // in the middle of a dataset generated before anyone needed it.
        assert_eq!(DAG_ACTIVATION_HEIGHT % EPOCH_LENGTH, 0);
        assert_eq!(epoch_of(DAG_ACTIVATION_HEIGHT), 1);
        assert_eq!(epoch_of(DAG_ACTIVATION_HEIGHT - 1), 0);
    }

    #[test]
    fn the_realised_sizes_sit_just_under_their_nominal_ceilings() {
        // Under, never over: the prime adjustment rounds down, so a miner
        // sizing an allocation from DATASET_BYTES always has room.
        assert!(cache_bytes() <= CACHE_BYTES);
        assert!(dataset_bytes() <= DATASET_BYTES);

        // And not far under — a bug in the prime search that returned 2 would
        // still satisfy the bound above.
        assert!(cache_bytes() > CACHE_BYTES - 64 * ITEM_BYTES);
        assert!(dataset_bytes() > DATASET_BYTES - 64 * MIX_BYTES);
    }

    #[test]
    fn the_cache_is_a_small_fraction_of_the_dataset() {
        // The property that keeps running a validator cheap. If this ratio ever
        // narrowed, light verification would stop being light.
        let ratio = dataset_bytes() / cache_bytes();
        assert!(
            ratio >= 60,
            "cache is 1/{ratio} of the dataset, expected ~1/64"
        );
    }

    #[test]
    fn the_dataset_holds_two_items_per_page() {
        assert_eq!(dataset_items(), dataset_pages() * 2);
        assert_eq!(MIX_BYTES, ITEM_BYTES * 2);
    }

    #[test]
    fn items_round_trip_through_their_byte_form() {
        let mut item = ZERO_ITEM;
        for (i, word) in item.iter_mut().enumerate() {
            *word = (i as u32).wrapping_mul(0x9E37_79B9);
        }
        assert_eq!(item_from_le_bytes(&item_to_le_bytes(&item)), item);
    }

    #[test]
    fn fnv_mixes_both_operands() {
        assert_ne!(fnv(1, 0), fnv(0, 0));
        assert_ne!(fnv(0, 1), fnv(0, 0));
        // Wrapping, not panicking: the multiply overflows constantly and a
        // debug build would abort on the first mix otherwise.
        let _ = fnv(u32::MAX, u32::MAX);
    }

    #[test]
    fn the_three_domain_keys_are_distinct() {
        // Three uses of one hash on structurally similar 64-byte inputs. Equal
        // keys would let a cache item and a dataset item collide.
        assert_ne!(cache_key(), item_key());
        assert_ne!(cache_key(), mix_key());
        assert_ne!(item_key(), mix_key());
    }
}
