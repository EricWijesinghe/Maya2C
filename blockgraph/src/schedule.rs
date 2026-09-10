//! Turning an agreed transaction order into waves that may run in parallel.
//!
//! # The rule
//!
//! For transaction `i`, with `conflicts(i, j)` meaning their shard sets
//! intersect:
//!
//! ```text
//! wave(i) = 0                                    if no j < i conflicts with i
//! wave(i) = 1 + max{ wave(j) : j < i, conflict } otherwise
//! ```
//!
//! Two properties follow, and together they are the whole correctness argument.
//!
//! **Within a wave, nothing conflicts.** If `i < j` are both in wave `w` and
//! share a shard, then when `j` was assigned, that shard had already been
//! claimed at wave `w`, so `wave(j) >= w + 1`. Contradiction.
//!
//! **Conflicting transactions keep their relative order.** If `i < j` conflict
//! then `wave(j) > wave(i)`, directly from the rule.
//!
//! So executing waves in ascending order — and transactions *within* a wave in
//! any order, on any number of threads — reaches the state the serial order
//! would have reached. That is the claim `serial_equivalence_tests.rs` checks
//! against an executable model, and it is why there is no two-phase commit
//! here: the order was agreed before execution began, so there is nothing left
//! to negotiate at commit time.
//!
//! # Cost
//!
//! One pass, `O(n)` with a 64-bit mask operation per transaction. The
//! `last_claimed` table is [`SHARD_COUNT`] words held on the stack, so
//! scheduling allocates only the output.
//!
//! # What this does not do
//!
//! It does not decide *whether* two transactions conflict — it is handed the
//! shard sets. A caller that computes an access set too narrowly gets a
//! schedule that runs them concurrently, and a wrong state root. Computing the
//! set is the node's job and is the part that has to be conservative: a
//! transaction whose accesses are not known exactly must declare more, never
//! fewer.

use alloc::vec::Vec;

use crate::error::{GraphError, Result};
use crate::shard::{SHARD_COUNT, ShardId};

/// The shards one transaction touches, as a bitmask.
///
/// A `u64` because [`SHARD_COUNT`] is 64: conflict detection is then a single
/// `AND`, which is what keeps scheduling linear rather than quadratic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    mask: u64,
}

impl Access {
    /// Builds an access set from a shard bitmask.
    ///
    /// # Errors
    ///
    /// [`GraphError::EmptyAccessSet`] for a zero mask. A transaction touching
    /// nothing would be conflict-free with everything and land in wave 0
    /// alongside transactions it may in fact depend on — the one input that
    /// makes this scheduler produce a wrong answer, so it is refused rather
    /// than scheduled.
    pub const fn from_mask(mask: u64) -> Result<Self> {
        if mask == 0 {
            return Err(GraphError::EmptyAccessSet);
        }
        Ok(Self { mask })
    }

    /// Builds an access set from the shards a transaction touches.
    ///
    /// # Errors
    ///
    /// [`GraphError::EmptyAccessSet`] if `shards` is empty.
    pub fn from_shards(shards: &[ShardId]) -> Result<Self> {
        let mut mask = 0u64;
        for shard in shards {
            mask |= 1u64 << shard.index();
        }
        Self::from_mask(mask)
    }

    /// The raw bitmask.
    #[must_use]
    pub const fn mask(self) -> u64 {
        self.mask
    }

    /// Whether this transaction touches `shard`.
    #[must_use]
    pub const fn touches(self, shard: ShardId) -> bool {
        self.mask & (1u64 << shard.index()) != 0
    }

    /// Whether two transactions contend for any shard.
    #[must_use]
    pub const fn conflicts_with(self, other: Self) -> bool {
        self.mask & other.mask != 0
    }
}

/// A set of transaction indices that may execute concurrently.
///
/// The indices are ascending, which is not cosmetic: a wave whose order
/// depended on a hash map's iteration order would be a different wave on a
/// different node, and although executing it in any order reaches the same
/// state, *logging* or *metering* it would not agree between nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wave {
    indices: Vec<usize>,
}

impl Wave {
    /// The transaction indices in this wave, ascending.
    #[must_use]
    pub fn indices(&self) -> &[usize] {
        &self.indices
    }

    /// How many transactions this wave carries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.indices.len()
    }

    /// Whether the wave is empty. Never true for a wave `schedule` produced.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// Partitions an ordered transaction list into parallel-executable waves.
///
/// `accesses[i]` is the shard set of the `i`-th transaction *in the agreed
/// order*. The returned waves are executed in sequence; within one, order is
/// free.
///
/// # Errors
///
/// Propagates nothing — the access sets were already validated by
/// [`Access::from_mask`] — but returns [`GraphError::EmptyAccessSet`] if handed
/// a default-constructed `Access` through some future path, so the invariant is
/// checked where it is relied on rather than assumed.
pub fn schedule(accesses: &[Access]) -> Result<Vec<Wave>> {
    // `last_claimed[s]` is one *past* the highest wave that has claimed shard
    // `s`, so 0 means untouched and the maximum over a transaction's shards is
    // its wave directly. Storing the wave itself would need a sentinel for
    // "never claimed" and a branch to handle it.
    let mut last_claimed = [0usize; SHARD_COUNT];
    let mut waves: Vec<Wave> = Vec::new();

    for (index, access) in accesses.iter().enumerate() {
        if access.mask == 0 {
            return Err(GraphError::EmptyAccessSet);
        }

        let mut wave = 0usize;
        let mut remaining = access.mask;
        while remaining != 0 {
            let shard = remaining.trailing_zeros() as usize;
            remaining &= remaining - 1;
            if last_claimed[shard] > wave {
                wave = last_claimed[shard];
            }
        }

        if wave == waves.len() {
            waves.push(Wave {
                indices: Vec::new(),
            });
        }
        waves[wave].indices.push(index);

        let mut remaining = access.mask;
        while remaining != 0 {
            let shard = remaining.trailing_zeros() as usize;
            remaining &= remaining - 1;
            last_claimed[shard] = wave + 1;
        }
    }

    Ok(waves)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn access(mask: u64) -> Access {
        Access::from_mask(mask).expect("non-zero")
    }

    #[test]
    fn a_zero_mask_is_refused() {
        assert_eq!(Access::from_mask(0), Err(GraphError::EmptyAccessSet));
        assert_eq!(Access::from_shards(&[]), Err(GraphError::EmptyAccessSet));
    }

    #[test]
    fn disjoint_transactions_all_land_in_one_wave() {
        let accesses = vec![access(0b0001), access(0b0010), access(0b0100)];
        let waves = schedule(&accesses).expect("valid");
        assert_eq!(waves.len(), 1);
        assert_eq!(waves[0].indices(), &[0, 1, 2]);
    }

    #[test]
    fn a_chain_of_conflicts_becomes_one_wave_each() {
        // Each touches the shard the previous one did.
        let accesses = vec![access(0b0011), access(0b0110), access(0b1100)];
        let waves = schedule(&accesses).expect("valid");
        assert_eq!(waves.len(), 3);
        for (wave, expected) in waves.iter().zip([0, 1, 2]) {
            assert_eq!(wave.indices(), &[expected]);
        }
    }

    #[test]
    fn conflicting_transactions_never_share_a_wave() {
        let accesses = vec![access(0b001), access(0b001), access(0b010)];
        let waves = schedule(&accesses).expect("valid");
        // 0 and 2 are disjoint, 1 conflicts with 0.
        assert_eq!(waves.len(), 2);
        assert_eq!(waves[0].indices(), &[0, 2]);
        assert_eq!(waves[1].indices(), &[1]);
    }

    #[test]
    fn an_empty_input_schedules_to_no_waves() {
        assert_eq!(schedule(&[]).expect("valid"), vec![]);
    }

    #[test]
    fn a_transaction_touching_every_shard_serialises_against_everything() {
        let accesses = vec![access(0b01), access(u64::MAX), access(0b10)];
        let waves = schedule(&accesses).expect("valid");
        assert_eq!(waves.len(), 3);
        assert_eq!(waves[0].indices(), &[0]);
        assert_eq!(waves[1].indices(), &[1]);
        assert_eq!(waves[2].indices(), &[2]);
    }

    #[test]
    fn every_transaction_appears_exactly_once() {
        let accesses: Vec<Access> = (0..64).map(|i| access(1u64 << (i % 7))).collect();
        let waves = schedule(&accesses).expect("valid");
        let mut seen: Vec<usize> = waves
            .iter()
            .flat_map(|wave| wave.indices().iter().copied())
            .collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..64).collect::<Vec<_>>());
    }

    #[test]
    fn indices_within_a_wave_ascend() {
        let accesses: Vec<Access> = (0..40).map(|i| access(1u64 << (i % 5))).collect();
        for wave in schedule(&accesses).expect("valid") {
            assert!(wave.indices().windows(2).all(|pair| pair[0] < pair[1]));
        }
    }

    #[test]
    fn shard_membership_round_trips_through_a_mask() {
        let shards = [crate::shard::shard_of(&[0u8; 32])];
        let set = Access::from_shards(&shards).expect("non-empty");
        assert!(set.touches(shards[0]));
    }
}
