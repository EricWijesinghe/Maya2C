//! The mining dataset: 4 GiB derived from the 64 MiB cache.
//!
//! ## The asymmetry this file exists to create
//!
//! Every dataset item is a pure function of the cache and its own index
//! ([`dataset_item`]). A miner materialises all 67 million of them once per
//! epoch and then reads pages at random; a validator materialises none of them
//! and recomputes the handful a block actually touched. Same function, two
//! footprints — 4 GiB against 64 MiB.
//!
//! That is what makes the work bandwidth-bound rather than arithmetic-bound.
//! Recomputing an item costs 256 cache reads and two BLAKE3 compressions, so an
//! implementation that tried to avoid holding the dataset pays roughly a
//! hundredfold in compute for what it saves in memory. Holding it is the only
//! sane strategy, and holding it means buying DRAM on the same open market
//! everyone else buys it on.
//!
//! ## Cost
//!
//! `2 × dataset_items()` BLAKE3 compressions plus `256 × dataset_items()` FNV
//! mixes — about 134 million compressions at [`Params::MAINNET`]. Generation is
//! spread across threads because the items are mutually independent given the
//! cache: item *i* never reads item *j*. Measured single-threaded this is
//! minutes; across eight threads it is tens of seconds, paid once per 5.21-day
//! epoch and paid ahead of the boundary rather than on it.
//!
//! ## Why the buffer is zeroed first
//!
//! [`Vec::resize`] writes 4 GiB of zeros before the real fill overwrites them.
//! That costs a fraction of a second against a generation measured in tens, and
//! it buys a `&mut [Item]` that can be split across threads with no `unsafe` at
//! all. Handing out uninitialised memory to eight threads to save 0.3 s in a
//! consensus-critical allocation is not a trade worth making.

use crate::crypto::dag::cache::Cache;
use crate::crypto::dag::{
    DATASET_PARENTS, ITEM_BYTES, Item, Params, ZERO_ITEM, blake3_512, fnv, fnv_item,
    item_from_le_bytes, item_key, item_to_le_bytes,
};
use crate::error::{NodeError, Result};

/// Words in one item. The parent walk indexes the mix modulo this.
const ITEM_WORDS: u32 = (ITEM_BYTES / 4) as u32;

/// Computes dataset item `index` from `cache`.
///
/// This is the whole definition of the dataset. [`Dataset::generate`] calls it
/// for every index; verification calls it for the ~128 indices a block touched.
/// Both must agree, on every machine, forever — so there is exactly one
/// implementation and both paths go through it.
///
/// `index` is taken modulo the cache length for its seed item, so no index is
/// out of range and the dataset is defined for every `u32`.
#[must_use]
pub fn dataset_item(cache: &Cache, index: u32) -> Item {
    let count = cache.len() as u32;
    let key = item_key();

    // Seed from one cache item, perturbed by the index so that two dataset
    // items sharing a seed item still diverge immediately.
    let mut mix = *cache.item((index % count) as usize);
    mix[0] ^= index;
    mix = item_from_le_bytes(&blake3_512(key, &item_to_le_bytes(&mix)));

    // Fold in 256 pseudo-randomly chosen cache items. The index is
    // data-dependent through `mix`, so the walk cannot be predicted ahead of
    // the work — which is what stops an implementation from prefetching its way
    // around holding the cache.
    for parent in 0..DATASET_PARENTS {
        let selector = fnv(index ^ parent, mix[(parent % ITEM_WORDS) as usize]) % count;
        mix = fnv_item(&mix, cache.item(selector as usize));
    }

    item_from_le_bytes(&blake3_512(key, &item_to_le_bytes(&mix)))
}

/// A materialised mining dataset for one epoch.
pub struct Dataset {
    epoch: u64,
    params: Params,
    pages: u32,
    items: Vec<Item>,
}

impl Dataset {
    /// Generates the dataset `cache` derives, across [`suggested_threads`]
    /// threads.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::DagAllocation`] if the backing store — 4 GiB at
    /// [`Params::MAINNET`] — cannot be reserved.
    pub fn generate(cache: &Cache) -> Result<Self> {
        Self::generate_with_threads(cache, suggested_threads())
    }

    /// Generates the dataset across exactly `threads` threads.
    ///
    /// Thread count is a parameter so that a test can pin it: a generator that
    /// only produced the right answer at one particular width would be a
    /// chunking bug waiting for a machine with a different core count, and the
    /// determinism tests pin both 1 and *n* against each other to catch it.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::DagAllocation`] if the backing store cannot be
    /// reserved.
    pub fn generate_with_threads(cache: &Cache, threads: usize) -> Result<Self> {
        let params = cache.params();
        let count = params.dataset_items() as usize;

        let mut items: Vec<Item> = Vec::new();
        items
            .try_reserve_exact(count)
            .map_err(|_| NodeError::DagAllocation {
                bytes: count.saturating_mul(ITEM_BYTES),
            })?;
        items.resize(count, ZERO_ITEM);

        let threads = threads.max(1);
        let chunk = count.div_ceil(threads).max(1);

        // Items are mutually independent given the cache, so the fill is a
        // partition of disjoint `&mut` slices and needs no synchronisation and
        // no shared state — the reason this parallelises at all.
        std::thread::scope(|scope| {
            for (ordinal, slice) in items.chunks_mut(chunk).enumerate() {
                let base = ordinal * chunk;
                scope.spawn(move || {
                    for (offset, slot) in slice.iter_mut().enumerate() {
                        // Cast is sound: `count` is at most 2^26 items.
                        *slot = dataset_item(cache, (base + offset) as u32);
                    }
                });
            }
        });

        Ok(Self {
            epoch: cache.epoch(),
            params,
            pages: cache.dataset_pages(),
            items,
        })
    }

    /// The epoch this dataset belongs to.
    #[must_use]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The parameters it was generated at.
    #[must_use]
    pub fn params(&self) -> Params {
        self.params
    }

    /// Number of 128-byte pages. Hashimoto reduces its lookups modulo this.
    #[must_use]
    pub fn pages(&self) -> u32 {
        self.pages
    }

    /// Number of 64-byte items held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the dataset is empty. Never true for a generated dataset.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The item at `index`, taken modulo the dataset length.
    #[must_use]
    pub fn item(&self, index: u32) -> &Item {
        &self.items[index as usize % self.items.len()]
    }

    /// The two adjacent items forming page `page`, taken modulo the page count.
    ///
    /// The 128-byte page is the unit a lookup reads: four 32-byte memory
    /// sectors, all of them wanted, which is what keeps a GPU's transactions
    /// fully utilised.
    #[must_use]
    pub fn page(&self, page: u32) -> [Item; 2] {
        let first = (page % self.pages) * 2;
        [*self.item(first), *self.item(first + 1)]
    }

    /// Every item, for parity checks against a GPU-generated dataset.
    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }
}

/// Threads to generate a dataset across.
///
/// Unlike the CPU miner's [`suggested_threads`], this is not capped: dataset
/// generation holds one shared cache and writes into disjoint slices of one
/// allocation, so a thread costs a stack and nothing else. The 32 MiB-per-
/// thread footprint that forces the miner's cap does not exist here.
///
/// [`suggested_threads`]: crate::consensus::miner::suggested_threads
#[must_use]
pub fn suggested_threads() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4 MiB of dataset from a 512 KiB cache. Builds in about a second even
    /// unoptimized, so the determinism properties below run on every
    /// `cargo test` rather than behind `--ignored` where nobody sees them.
    const TEST: Params = Params::TESTING;

    fn cache(epoch: u64) -> Cache {
        Cache::generate(epoch, TEST).expect("cache generation must succeed")
    }

    #[test]
    fn a_dataset_is_a_pure_function_of_its_cache() {
        // The property node validation and the GPU miner both depend on: the
        // same cache must yield the same bytes, or a miner's solution will not
        // reproduce on the validator that checks it.
        let cache = cache(0);
        let first = Dataset::generate(&cache).expect("generation must succeed");
        let second = Dataset::generate(&cache).expect("generation must succeed");

        assert_eq!(first.len(), second.len());
        assert_eq!(
            first.items(),
            second.items(),
            "dataset is not deterministic"
        );
    }

    #[test]
    fn generation_is_independent_of_how_many_threads_did_it() {
        // A chunking bug would show up as a machine-dependent consensus split:
        // the eight-core node accepts a block the four-core node rejects.
        let cache = cache(0);
        let serial = Dataset::generate_with_threads(&cache, 1).expect("generation must succeed");

        for threads in [2usize, 3, 7, 16] {
            let parallel =
                Dataset::generate_with_threads(&cache, threads).expect("generation must succeed");
            assert_eq!(
                serial.items(),
                parallel.items(),
                "dataset differs at {threads} threads"
            );
        }
    }

    #[test]
    fn every_materialised_item_equals_its_recomputation_from_the_cache() {
        // The asymmetry itself. A miner reads item i out of 4 GiB; a validator
        // recomputes it from 64 MiB. If these two ever disagreed, every block
        // would be rejected by everyone who did not mine it.
        let cache = cache(0);
        let dataset = Dataset::generate(&cache).expect("generation must succeed");

        for index in 0..dataset.len() as u32 {
            assert_eq!(
                *dataset.item(index),
                dataset_item(&cache, index),
                "item {index} does not match its recomputation"
            );
        }
    }

    #[test]
    fn a_dataset_holds_no_empty_items() {
        let cache = cache(0);
        let dataset = Dataset::generate(&cache).expect("generation must succeed");
        assert!(dataset.items().iter().all(|item| *item != ZERO_ITEM));
    }

    #[test]
    fn an_epoch_change_rewrites_essentially_every_item() {
        let zero = Dataset::generate(&cache(0)).expect("generation must succeed");
        let one = Dataset::generate(&cache(1)).expect("generation must succeed");

        assert_eq!(zero.len(), one.len(), "dataset size must not drift");

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
    fn a_page_is_the_two_items_that_sit_at_it() {
        let cache = cache(0);
        let dataset = Dataset::generate(&cache).expect("generation must succeed");

        for page in [0u32, 1, 17, dataset.pages() - 1] {
            let [first, second] = dataset.page(page);
            assert_eq!(first, *dataset.item(page * 2));
            assert_eq!(second, *dataset.item(page * 2 + 1));
            assert_ne!(first, second);
        }
    }

    #[test]
    fn page_and_item_indices_wrap_rather_than_panicking() {
        // Hashimoto already reduces modulo the page count, but a lookup that
        // could panic inside block validation would be a denial of service with
        // extra steps.
        let cache = cache(0);
        let dataset = Dataset::generate(&cache).expect("generation must succeed");

        assert_eq!(dataset.page(dataset.pages()), dataset.page(0));
        assert_eq!(*dataset.item(u32::MAX), *dataset.item(u32::MAX));
    }

    #[test]
    fn the_dataset_carries_the_parameters_of_its_cache() {
        let cache = cache(3);
        let dataset = Dataset::generate(&cache).expect("generation must succeed");

        assert_eq!(dataset.epoch(), 3);
        assert_eq!(dataset.params(), TEST);
        assert_eq!(dataset.pages(), TEST.dataset_pages());
        assert_eq!(dataset.len(), TEST.dataset_items() as usize);
    }
}
