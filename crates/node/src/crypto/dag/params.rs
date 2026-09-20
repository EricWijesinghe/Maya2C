//! Sizing parameters for the DAG, and the prime search that fixes them.
//!
//! ## Why the sizes are a value and not only a constant
//!
//! Consensus runs at [`Params::MAINNET`]: a 64 MiB cache and a 4 GiB dataset.
//! Nothing else may run there, and no code path here can select a different
//! size for a block on the real chain — the chain's parameters are pinned in
//! [`crate::consensus::chain::ChainConfig`] and default to `MAINNET`.
//!
//! Tests are the reason the sizes are a value at all. A determinism test wants
//! to build *whole* datasets and compare them, twice, from two epochs, and a CI
//! runner cannot hold 8 GiB to do it. [`Params::TESTING`] is the same
//! construction at 1/64th the cache and 1/512th the dataset, which builds in
//! milliseconds and exercises every branch the mainnet sizes do. The alternative
//! — testing the 4 GiB path only under `#[ignore]` — means the determinism
//! property is checked by hand, on someone's workstation, if at all.
//!
//! ## Why the counts are prime
//!
//! Both counts are rounded *down* to a prime. A composite count shares factors
//! with the strides the mixing function produces, which lets the walk fall into
//! cycles far shorter than the array: the cache would be nominally 64 MiB and
//! effectively a fraction of it, the memory-hardness claim would be false, and
//! every test would still pass. Ethash rounds to a prime for the same reason.

use crate::crypto::dag::{CACHE_BYTES, DATASET_BYTES, EPOCH_LENGTH, ITEM_BYTES, MIX_BYTES};

/// The sizes one DAG instance is built at.
///
/// `Copy`, so it can live inside `ChainConfig` without changing that type's
/// shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    /// Blocks between dataset regenerations. Treated as 1 if zero.
    pub epoch_length: u64,
    /// Nominal cache size in bytes, before the prime adjustment.
    pub cache_bytes: usize,
    /// Nominal dataset size in bytes, before the prime adjustment.
    pub dataset_bytes: usize,
}

impl Params {
    /// Consensus parameters: 64 MiB cache, 4 GiB dataset, 30,000-block epochs.
    pub const MAINNET: Self = Self {
        epoch_length: EPOCH_LENGTH,
        cache_bytes: CACHE_BYTES,
        dataset_bytes: DATASET_BYTES,
    };

    /// Test parameters: 512 KiB cache, 4 MiB dataset, 8-block epochs.
    ///
    /// Small enough that a test can build two full datasets and compare them
    /// byte for byte, and that an epoch boundary is a few blocks away rather
    /// than five days. The ratio between cache and dataset is deliberately kept
    /// at 1:8 rather than mainnet's 1:64 so that the dataset stays a multiple of
    /// the cache without either being trivially small.
    ///
    /// The sizes are also a time budget. Dataset generation is 256 cache reads
    /// per item, and in an unoptimized test build that is real seconds — these
    /// figures put the DAG unit suite at about eight of them, which is the most
    /// a property this important should have to justify.
    pub const TESTING: Self = Self {
        epoch_length: 8,
        cache_bytes: 512 * 1024,
        dataset_bytes: 4 * 1024 * 1024,
    };

    /// Number of 64-byte items in the cache. Prime; see the module docs.
    #[must_use]
    pub fn cache_items(&self) -> u32 {
        // Sizes are well inside u32 for any parameter set that fits in memory.
        let nominal = u32::try_from(self.cache_bytes / ITEM_BYTES).unwrap_or(u32::MAX);
        largest_prime_at_most(nominal)
    }

    /// Number of 128-byte pages in the dataset. Prime; see the module docs.
    #[must_use]
    pub fn dataset_pages(&self) -> u32 {
        let nominal = u32::try_from(self.dataset_bytes / MIX_BYTES).unwrap_or(u32::MAX);
        largest_prime_at_most(nominal)
    }

    /// Number of 64-byte items in the dataset. Two per page.
    #[must_use]
    pub fn dataset_items(&self) -> u32 {
        // Cannot overflow: `dataset_pages` is at most 2^25 for a 4 GiB dataset.
        self.dataset_pages() * 2
    }

    /// Realised cache size in bytes, after the prime adjustment.
    #[must_use]
    pub fn realised_cache_bytes(&self) -> usize {
        self.cache_items() as usize * ITEM_BYTES
    }

    /// Realised dataset size in bytes, after the prime adjustment.
    #[must_use]
    pub fn realised_dataset_bytes(&self) -> usize {
        self.dataset_pages() as usize * MIX_BYTES
    }

    /// The epoch a block at `height` belongs to.
    ///
    /// A zero `epoch_length` is treated as one rather than dividing by zero: a
    /// misconfigured parameter set should degenerate into "every block is its
    /// own epoch", which is slow and obvious, not a panic inside validation.
    #[must_use]
    pub fn epoch_of(&self, height: u64) -> u64 {
        height / self.epoch_length.max(1)
    }
}

/// The largest prime not exceeding `n`, for `n >= 2`.
///
/// Trial division: the candidates here are around 2²⁰ and 2²⁵, so the divisor
/// loop runs to about 1,024 and 5,793 respectively, and the whole search
/// finishes in microseconds. A sieve would be faster asymptotically and would
/// need megabytes to answer a question asked a handful of times per process.
pub(crate) fn largest_prime_at_most(n: u32) -> u32 {
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
pub(crate) fn is_prime(n: u32) -> bool {
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn primality_agrees_with_a_known_list() {
        let primes = [
            2u32, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 97, 65_537, 1_299_709,
        ];
        for p in primes {
            assert!(is_prime(p), "{p} reported composite");
        }
        for c in [0u32, 1, 4, 9, 15, 21, 25, 91, 65_535, 1_299_707] {
            assert!(!is_prime(c), "{c} reported prime");
        }
    }

    #[test]
    fn the_largest_prime_search_lands_on_the_right_value() {
        assert_eq!(largest_prime_at_most(100), 97);
        assert_eq!(largest_prime_at_most(97), 97);
        assert_eq!(largest_prime_at_most(96), 89);
        assert_eq!(largest_prime_at_most(3), 3);
        assert_eq!(largest_prime_at_most(2), 2);
    }

    #[test]
    fn both_parameter_sets_produce_prime_counts() {
        for params in [Params::MAINNET, Params::TESTING] {
            assert!(is_prime(params.cache_items()), "cache count is composite");
            assert!(
                is_prime(params.dataset_pages()),
                "dataset page count is composite"
            );
        }
    }

    #[test]
    fn realised_sizes_sit_just_under_their_nominal_ceilings() {
        // Under, never over: the prime adjustment rounds down, so a miner
        // sizing an allocation from the nominal figure always has room.
        for params in [Params::MAINNET, Params::TESTING] {
            assert!(params.realised_cache_bytes() <= params.cache_bytes);
            assert!(params.realised_dataset_bytes() <= params.dataset_bytes);

            // And not far under — a prime search that returned 2 would still
            // satisfy the bound above.
            assert!(params.realised_cache_bytes() > params.cache_bytes - 64 * ITEM_BYTES);
            assert!(params.realised_dataset_bytes() > params.dataset_bytes - 64 * MIX_BYTES);
        }
    }

    #[test]
    fn the_test_parameters_are_small_enough_to_build_in_a_unit_test() {
        // The property that makes whole-dataset determinism testable in CI. If
        // this ever grew past a few megabytes the tests that build two datasets
        // would start timing runners out instead of failing honestly.
        assert!(Params::TESTING.realised_dataset_bytes() <= 4 * 1024 * 1024);
        assert!(Params::TESTING.realised_cache_bytes() <= 512 * 1024);
    }

    #[test]
    fn a_zero_epoch_length_degenerates_rather_than_dividing_by_zero() {
        let broken = Params {
            epoch_length: 0,
            ..Params::TESTING
        };
        assert_eq!(broken.epoch_of(0), 0);
        assert_eq!(broken.epoch_of(7), 7);
    }

    #[test]
    fn epochs_partition_the_height_space() {
        let params = Params::TESTING;
        assert_eq!(params.epoch_of(0), 0);
        assert_eq!(params.epoch_of(params.epoch_length - 1), 0);
        assert_eq!(params.epoch_of(params.epoch_length), 1);
    }
}
