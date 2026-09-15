//! Kani harnesses for the bounds and the scheduler.
//!
//! Run them with:
//!
//! ```text
//! cargo kani -p maya-blockgraph
//! ```
//!
//! # Bounded versus unbounded
//!
//! - [`shard_of`] and [`Access::from_mask`] are proved **unbounded**: every
//!   address byte, every `u64` mask.
//! - Everything folding over a list carries `#[kani::unwind]` and is proved at
//!   [`PROVED_LENGTH`] only. Those are **bounded** proofs.
//!
//! The scheduler harnesses are the ones worth reading. They restate the two
//! properties the crate documentation argues informally — no conflict shares a
//! wave, and conflicting transactions keep their order — as post-conditions
//! over symbolic access sets, so a change that broke either fails here rather
//! than in whichever integration test happened to generate a witness.
//!
//! The masks are narrowed to [`PROVED_SHARDS`] bits. Conflict is a bitwise
//! `AND` and the scheduler's loop is over set bits, so a mask wider than the
//! number of transactions adds unreachable bits rather than new cases — and
//! `u64`-wide symbolic masks are what makes the solver expensive.
//!
//! [`shard_of`]: crate::shard::shard_of
//! [`Access::from_mask`]: crate::schedule::Access::from_mask

use alloc::vec::Vec;

use crate::batch::{Batch, MAX_BATCH_TRANSACTIONS};
use crate::error::GraphError;
use crate::refs::{BatchId, BatchRefs, MAX_REFS_PER_BLOCK};
use crate::schedule::{Access, schedule};
use crate::shard::{SHARD_COUNT, ShardId, shard_of};
use crate::shard_manager::{MAX_PREFIX_BITS, Prefix, ShardMap};

/// List length the folds are proved over.
const PROVED_LENGTH: usize = 3;

/// Distinct shards the scheduler is proved over.
const PROVED_SHARDS: u32 = 3;

/// A symbolic access set inside the narrowed shard range.
fn any_access() -> Access {
    let mask: u64 = kani::any();
    kani::assume(mask != 0);
    kani::assume(mask < (1u64 << PROVED_SHARDS));
    Access::from_mask(mask).expect("non-zero by assumption")
}

/// `shard_of` always names a partition that exists.
///
/// Unbounded over the whole address space: only the first byte is read, so
/// quantifying over it quantifies over every address.
#[kani::proof]
fn every_address_lands_in_a_partition_that_exists() {
    let first: u8 = kani::any();
    let mut address = [0u8; 32];
    address[0] = first;

    assert!(shard_of(&address).index() < SHARD_COUNT);
}

/// `Access::from_mask` refuses exactly the empty set.
///
/// Unbounded over `u64`.
#[kani::proof]
fn an_access_set_is_refused_exactly_when_it_is_empty() {
    let mask: u64 = kani::any();

    match Access::from_mask(mask) {
        Ok(access) => {
            assert!(mask != 0);
            assert!(access.mask() == mask);
        }
        Err(error) => {
            assert!(mask == 0);
            assert!(error == GraphError::EmptyAccessSet);
        }
    }
}

/// Conflict is symmetric, and a set always conflicts with itself.
///
/// Unbounded over both masks. Symmetry is what lets the scheduler check a new
/// transaction against claimed shards rather than against every predecessor.
#[kani::proof]
fn conflict_is_symmetric_and_reflexive() {
    let left = {
        let mask: u64 = kani::any();
        kani::assume(mask != 0);
        Access::from_mask(mask).expect("non-zero by assumption")
    };
    let right = {
        let mask: u64 = kani::any();
        kani::assume(mask != 0);
        Access::from_mask(mask).expect("non-zero by assumption")
    };

    assert!(left.conflicts_with(right) == right.conflicts_with(left));
    assert!(left.conflicts_with(left));
}

/// `BatchRefs::new` accepts exactly the well-formed sets.
///
/// Bounded at [`PROVED_LENGTH`] identifiers, each narrowed to one varying byte
/// — ordering compares lexicographically from the front, so a differing first
/// byte decides every comparison the rule makes.
#[kani::proof]
#[kani::unwind(5)]
fn a_reference_set_is_accepted_exactly_when_it_is_well_formed() {
    let length: usize = kani::any();
    kani::assume(length <= PROVED_LENGTH);

    let mut ids: Vec<BatchId> = Vec::with_capacity(length);
    for _ in 0..length {
        let leading: u8 = kani::any();
        let mut id = [0u8; 32];
        id[0] = leading;
        ids.push(id);
    }

    let ascending = ids.windows(2).all(|pair| pair[0] < pair[1]);
    let bounded = ids.len() <= MAX_REFS_PER_BLOCK;

    match BatchRefs::new(ids.clone()) {
        Ok(refs) => {
            assert!(ascending);
            assert!(bounded);
            assert!(refs.len() == ids.len());
        }
        Err(GraphError::TooManyRefs) => assert!(!bounded),
        Err(GraphError::RefsNotStrictlyAscending) => assert!(!ascending),
        Err(_) => unreachable!("`new` returns no other variant"),
    }
}

/// A batch is accepted exactly when it is non-empty, bounded, and carries no
/// zero-length transaction.
///
/// Bounded at [`PROVED_LENGTH`] transactions of at most two bytes. The byte
/// ceiling is far out of reach at that size, so this harness covers the count
/// and emptiness rules; the byte rule is covered by the unit tests, which can
/// afford an 8 MiB input.
#[kani::proof]
#[kani::unwind(5)]
fn a_batch_is_accepted_exactly_when_it_is_well_formed() {
    let length: usize = kani::any();
    kani::assume(length <= PROVED_LENGTH);

    let mut transactions: Vec<Vec<u8>> = Vec::with_capacity(length);
    for _ in 0..length {
        let size: usize = kani::any();
        kani::assume(size <= 2);
        transactions.push(alloc::vec![0u8; size]);
    }

    let non_empty = !transactions.is_empty();
    let bounded = transactions.len() <= MAX_BATCH_TRANSACTIONS;
    let all_populated = transactions.iter().all(|t| !t.is_empty());

    assert!(Batch::new(transactions).is_ok() == (non_empty && bounded && all_populated));
}

/// No wave produced by the scheduler contains two conflicting transactions.
///
/// Bounded at [`PROVED_LENGTH`] transactions over [`PROVED_SHARDS`] shards.
/// This is the first half of the correctness argument in
/// [`crate::schedule`](fn@crate::schedule), stated as a post-condition.
#[kani::proof]
#[kani::unwind(5)]
fn no_wave_holds_two_conflicting_transactions() {
    let mut accesses: Vec<Access> = Vec::with_capacity(PROVED_LENGTH);
    for _ in 0..PROVED_LENGTH {
        accesses.push(any_access());
    }

    let waves = schedule(&accesses).expect("every mask is non-zero");

    for wave in &waves {
        let indices = wave.indices();
        for (position, &left) in indices.iter().enumerate() {
            for &right in &indices[position + 1..] {
                assert!(!accesses[left].conflicts_with(accesses[right]));
            }
        }
    }
}

/// Conflicting transactions land in strictly increasing waves, and every
/// transaction is scheduled exactly once.
///
/// Bounded at [`PROVED_LENGTH`]. The second half of the correctness argument,
/// plus the totality property that stops a scheduler from being "correct" by
/// dropping the transactions it could not place.
#[kani::proof]
#[kani::unwind(5)]
fn conflicts_are_ordered_and_nothing_is_dropped() {
    let mut accesses: Vec<Access> = Vec::with_capacity(PROVED_LENGTH);
    for _ in 0..PROVED_LENGTH {
        accesses.push(any_access());
    }

    let waves = schedule(&accesses).expect("every mask is non-zero");

    // Totality: each index appears in exactly one wave.
    let mut wave_of = [usize::MAX; PROVED_LENGTH];
    let mut placed = 0usize;
    for (number, wave) in waves.iter().enumerate() {
        for &index in wave.indices() {
            assert!(wave_of[index] == usize::MAX, "scheduled twice");
            wave_of[index] = number;
            placed += 1;
        }
    }
    assert!(placed == PROVED_LENGTH);

    // Ordering: an earlier conflicting transaction is in an earlier wave.
    for left in 0..PROVED_LENGTH {
        for right in left + 1..PROVED_LENGTH {
            if accesses[left].conflicts_with(accesses[right]) {
                assert!(wave_of[left] < wave_of[right]);
            }
        }
    }
}

/// The two children of any prefix tile exactly their parent.
///
/// Unbounded over every canonical prefix above the deepest level: the whole
/// argument that a split keeps a map a tiling.
#[kani::proof]
fn children_tile_their_parent() {
    let bits: u32 = kani::any();
    let depth: u8 = kani::any();
    kani::assume(depth < MAX_PREFIX_BITS);
    if let Ok(prefix) = Prefix::new(bits, depth) {
        let (low, high) = prefix.children().expect("depth is below the limit");
        assert!(low.parent() == Some(prefix) && high.parent() == Some(prefix));
        assert!(low.start() == prefix.start());
        assert!(high.start() == low.start() + low.span());
        assert!(low.span() + high.span() == prefix.span());
    }
}

/// Every address lies in exactly one leaf of a four-leaf map, and `locate`
/// names that leaf.
///
/// Unbounded over the key bits, which are all `locate` reads.
#[kani::proof]
#[kani::unwind(6)]
fn every_address_is_located_in_the_one_leaf_containing_it() {
    let map = ShardMap::uniform(2).expect("four leaves");
    let mut address = [0u8; 32];
    for byte in address.iter_mut().take(4) {
        *byte = kani::any();
    }

    let containing = map
        .leaves()
        .iter()
        .filter(|leaf| leaf.contains(&address))
        .count();
    assert!(containing == 1);
    let located = map.prefix(map.locate(&address));
    assert!(located.is_some_and(|leaf| leaf.contains(&address)));
}

/// Splitting any leaf of a four-leaf map and merging it back is the identity.
#[kani::proof]
#[kani::unwind(8)]
fn a_split_undone_by_a_merge_restores_the_map() {
    let map = ShardMap::uniform(2).expect("four leaves");
    let index: usize = kani::any();
    kani::assume(index < 4);
    let shard = ShardId::new(index).expect("in range");

    let split = map.split(shard).expect("depth two can split");
    assert!(split.shard_count() == 5);
    assert!(split.merge(shard).expect("the children are siblings") == map);
}
