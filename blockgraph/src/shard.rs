//! Which of the 64 partitions an account's state lives in.
//!
//! # Shards are not threads
//!
//! Worth separating, because the two get conflated. A *shard* is a partition of
//! state; a *thread* is an execution resource. This module defines the first.
//! [`crate::schedule`](fn@crate::schedule) decides what may run concurrently, and how many threads
//! actually serve a wave is a deployment question that changes no result — the
//! state root is the same on one core and on sixty-four.

/// Number of state partitions.
///
/// A power of two so that [`shard_of`] is a shift rather than a modulo: `%` on
/// a non-power-of-two would bias the last bucket, and a partition that holds
/// more accounts than its neighbours is a partition that serialises more
/// transactions than its neighbours.
pub const SHARD_COUNT: usize = 64;

/// Bits of the address consumed by the partition: `log2(SHARD_COUNT)`.
const SHARD_BITS: u32 = SHARD_COUNT.trailing_zeros();

/// A state partition, in `0..`[`SHARD_COUNT`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShardId(u8);

impl ShardId {
    /// The partition index.
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// The partition an address belongs to.
///
/// # Why the high bits of the first byte
///
/// A Maya2C address is already a BLAKE3 digest of a public key, so every bit of
/// it is uniform and any six would do. Taking the *high* six of byte zero means
/// the partition is a prefix of the address, which makes a shard a contiguous
/// key range in RocksDB rather than a scattered one — an iteration over a shard
/// is then a range scan instead of a full sweep with a filter.
///
/// This is consensus. Two nodes partitioning differently would schedule
/// differently, and while [`crate::schedule`](fn@crate::schedule) guarantees any schedule it
/// produces agrees with the serial order, that guarantee is per-node: it does
/// not make two *different* partitions produce the same conflict set, and a
/// validator that thought two transactions were independent when another
/// thought they conflicted would compute a different state root.
#[must_use]
pub const fn shard_of(address: &[u8; 32]) -> ShardId {
    ShardId(address[0] >> (8 - SHARD_BITS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeSet;

    #[test]
    fn every_shard_index_is_inside_the_partition_count() {
        for first in 0..=u8::MAX {
            let mut address = [0u8; 32];
            address[0] = first;
            assert!(shard_of(&address).index() < SHARD_COUNT);
        }
    }

    #[test]
    fn all_sixty_four_partitions_are_reachable() {
        let mut seen = BTreeSet::new();
        for first in 0..=u8::MAX {
            let mut address = [0u8; 32];
            address[0] = first;
            seen.insert(shard_of(&address).index());
        }
        assert_eq!(seen.len(), SHARD_COUNT);
    }

    #[test]
    fn the_partition_is_a_prefix_so_each_shard_is_a_contiguous_range() {
        // Every first byte mapping to shard k must be consecutive. That is the
        // property making a shard a RocksDB range scan rather than a sweep.
        let mut previous = shard_of(&[0u8; 32]).index();
        let mut transitions = 0;
        for first in 1..=u8::MAX {
            let mut address = [0u8; 32];
            address[0] = first;
            let current = shard_of(&address).index();
            if current != previous {
                transitions += 1;
                assert_eq!(current, previous + 1, "shards must ascend by one");
            }
            previous = current;
        }
        assert_eq!(transitions, SHARD_COUNT - 1);
    }

    #[test]
    fn only_the_first_byte_decides_the_partition() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        a[0] = 0xC3;
        b[0] = 0xC3;
        b[31] = 0xFF;
        assert_eq!(shard_of(&a), shard_of(&b));
    }
}
