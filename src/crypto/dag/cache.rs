//! The verification cache: what every node holds, and what the dataset is
//! derived from.
//!
//! ## Why a cache exists at all
//!
//! A miner needs the full 4 GiB dataset resident, because it reads pages of it
//! at random and recomputing one on demand would cost far more than the memory
//! it saves. A *validator* reads only [`ACCESSES`](super::ACCESSES) pages per block, so it can
//! afford to recompute each one — and recomputing needs nothing but this
//! ~64 MiB cache.
//!
//! That asymmetry is the whole reason the design is usable. Without it, running
//! a node would mean holding 4 GiB resident to check other people's work, and
//! the set of machines that can validate the chain would collapse to the set
//! that can mine it.
//!
//! ## Construction
//!
//! Sequentially memory-hard, following Ethash's use of Lerner's RandMemoHash:
//!
//! ```text
//! cache[0] = H(seed)
//! cache[i] = H(cache[i-1])                       for i in 1..n
//!
//! repeat CACHE_ROUNDS times:
//!     for i in 0..n:
//!         v        = cache[i][0] mod n
//!         cache[i] = H(cache[(i-1+n) mod n] ⊕ cache[v])
//! ```
//!
//! The first pass is a plain chain — cheap, and enough to fill the array with
//! material that depends on the seed. The rounds are what make it hard: each
//! rewrite reads a *data-dependent* index, so the whole array has to be present
//! for any of it to be reproduced. An implementation that tried to recompute
//! cache items on demand would find each one depends on an unpredictable
//! earlier one, transitively on most of the array.
//!
//! Three rounds, as Ethash uses. The cost is 4 × n hashes — around 4.2 million
//! for a 64 MiB cache. Measured on this codebase: **0.9 s in release, 4.4 s in
//! a dev build**. Paid once per [`EPOCH_LENGTH`] blocks, which is 5.21 days, so
//! even the debug figure is four orders of magnitude inside its budget.
//!
//! [`EPOCH_LENGTH`]: crate::crypto::dag::EPOCH_LENGTH

use crate::crypto::dag::{
    CACHE_ROUNDS, ITEM_BYTES, Item, Params, ZERO_ITEM, blake3_512, cache_key, epoch_seed,
    item_from_le_bytes, item_to_le_bytes,
};
use crate::error::{NodeError, Result};

/// A generated verification cache for one epoch.
///
/// Holds items rather than raw bytes: every consumer addresses them as
/// sixteen-word units, and converting on each access would put a byte shuffle
/// inside the dataset generator's innermost loop.
///
/// Carries the [`Params`] it was built at, and the dataset page count derived
/// from them. A cache and the dataset derived from it must agree on both, and
/// carrying them means a caller cannot pair a cache with the wrong sizes — the
/// failure that would produce would be two nodes disagreeing about a block,
/// which is the worst class of bug this design can have.
#[derive(Clone)]
pub struct Cache {
    epoch: u64,
    params: Params,
    dataset_pages: u32,
    items: Vec<Item>,
}

impl Cache {
    /// Generates the cache for `epoch` at `params`.
    ///
    /// Costs roughly `4 × params.cache_items()` BLAKE3 compressions — around
    /// 4.2 million at [`Params::MAINNET`], measured at 0.9 s in release. Paid
    /// once per epoch, and a node can generate the next epoch's cache ahead of
    /// the boundary rather than stalling on it.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::DagAllocation`] if the ~64 MiB backing store cannot
    /// be reserved. Allocation is attempted up front with `try_reserve` rather
    /// than left to the allocator's abort path: a validator that cannot spare
    /// the memory should report it, not die.
    pub fn generate(epoch: u64, params: Params) -> Result<Self> {
        let count = params.cache_items() as usize;
        let key = cache_key();

        let mut items: Vec<Item> = Vec::new();
        items
            .try_reserve_exact(count)
            .map_err(|_| NodeError::DagAllocation {
                bytes: count * ITEM_BYTES,
            })?;

        // Pass one: a plain hash chain seeded by the epoch.
        let mut previous = blake3_512(key, &epoch_seed(epoch));
        items.push(item_from_le_bytes(&previous));
        for _ in 1..count {
            previous = blake3_512(key, &previous);
            items.push(item_from_le_bytes(&previous));
        }

        // Passes two and after: data-dependent rewrites. This is the part that
        // makes the array irreducible.
        for _ in 0..CACHE_ROUNDS {
            for index in 0..count {
                // Wraps to the last item at index 0, so every item has a
                // predecessor and the dependency graph has no entry point that
                // skips the tail.
                let previous_index = if index == 0 { count - 1 } else { index - 1 };
                let scrambled = items[index][0] as usize % count;

                let mut mixed = ZERO_ITEM;
                for (slot, (a, b)) in mixed
                    .iter_mut()
                    .zip(items[previous_index].iter().zip(items[scrambled].iter()))
                {
                    *slot = a ^ b;
                }

                items[index] = item_from_le_bytes(&blake3_512(key, &item_to_le_bytes(&mixed)));
            }
        }

        Ok(Self {
            epoch,
            params,
            dataset_pages: params.dataset_pages(),
            items,
        })
    }

    /// The epoch this cache was generated for.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The parameters this cache was generated at.
    #[must_use]
    pub fn params(&self) -> Params {
        self.params
    }

    /// Pages in the dataset this cache derives, precomputed at generation.
    ///
    /// Hashimoto reduces a mix word modulo this on every one of its
    /// [`ACCESSES`] lookups, so it must not be a prime search away.
    ///
    /// [`ACCESSES`]: crate::crypto::dag::ACCESSES
    #[must_use]
    pub fn dataset_pages(&self) -> u32 {
        self.dataset_pages
    }

    /// Number of items held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the cache is empty. Never true for a generated cache.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The item at `index`, which must be in range.
    #[must_use]
    pub fn item(&self, index: usize) -> &Item {
        &self.items[index]
    }

    /// The items, for the dataset generator.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::dag::{CACHE_BYTES, cache_bytes, cache_items};

    /// The cache these tests build. 512 KiB, which generates in milliseconds, so
    /// the determinism properties run on every `cargo test` rather than behind
    /// `--ignored` where nobody sees them. The mainnet-sized equivalents are
    /// kept below and marked.
    const TEST: Params = Params::TESTING;

    #[test]
    fn the_cache_is_sized_from_the_prime_item_count() {
        assert_eq!(cache_bytes(), cache_items() as usize * ITEM_BYTES);
        assert!(cache_bytes() <= CACHE_BYTES);
    }

    #[test]
    fn a_cache_carries_the_parameters_it_was_built_at() {
        let cache = Cache::generate(0, TEST).expect("generation must succeed");
        assert_eq!(cache.params(), TEST);
        assert_eq!(cache.len(), TEST.cache_items() as usize);
        assert_eq!(cache.dataset_pages(), TEST.dataset_pages());
    }

    #[test]
    fn a_cache_is_a_pure_function_of_its_epoch() {
        // The property the whole design rests on: two nodes at the same height
        // must derive byte-identical caches, or they will disagree about
        // whether a block is valid.
        let first = Cache::generate(0, TEST).expect("generation must succeed");
        let second = Cache::generate(0, TEST).expect("generation must succeed");

        assert_eq!(first.len(), second.len());
        assert_eq!(first.items(), second.items(), "cache is not deterministic");
    }

    #[test]
    fn different_epochs_produce_different_caches() {
        let zero = Cache::generate(0, TEST).expect("generation must succeed");
        let one = Cache::generate(1, TEST).expect("generation must succeed");

        assert_eq!(zero.len(), one.len(), "the cache size must not drift");
        assert_ne!(
            zero.items(),
            one.items(),
            "an epoch change must rotate the cache"
        );

        // Not merely different in one place: a seed change must reach
        // essentially every item, or the rotation is cosmetic.
        let shared = zero
            .items()
            .iter()
            .zip(one.items())
            .filter(|(a, b)| a == b)
            .count();
        assert!(
            shared < zero.len() / 1000,
            "{shared} of {} items survived the epoch change",
            zero.len()
        );
    }

    #[test]
    fn the_cache_holds_no_repeated_or_empty_items() {
        let cache = Cache::generate(0, TEST).expect("generation must succeed");

        assert!(cache.items().iter().all(|item| *item != ZERO_ITEM));

        // A sample rather than a full pairwise scan: the failure this guards
        // against is a construction that collapses to a short cycle, which
        // shows up in any window.
        let window = &cache.items()[..4096];
        let mut sorted: Vec<&Item> = window.iter().collect();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), window.len(), "the cache repeats itself");
    }

    #[test]
    #[ignore = "generates a 64 MiB cache; run with --ignored"]
    fn the_mainnet_cache_generates_and_is_deterministic() {
        // The properties above at the size consensus actually uses. Separate
        // and ignored because it is seconds rather than milliseconds, but it
        // has to exist: the small parameter set shares every line of code with
        // this one except the sizes, and the sizes are what allocate.
        let cache = Cache::generate(0, Params::MAINNET).expect("generation must succeed");
        assert_eq!(cache.len(), cache_items() as usize);
        assert_eq!(cache.dataset_pages(), Params::MAINNET.dataset_pages());

        let again = Cache::generate(0, Params::MAINNET).expect("generation must succeed");
        assert_eq!(cache.items(), again.items(), "cache is not deterministic");
    }
}
