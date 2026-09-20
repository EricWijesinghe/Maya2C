//! What a block commits to: a canonically ordered set of batch identifiers.
//!
//! This is the change that buys the throughput. A block that carried its
//! transactions costs every peer the full bytes at propagation time, on the
//! critical path, after the block was found. A block that carries *references*
//! costs 32 bytes per batch, and the transactions themselves travelled earlier,
//! off the critical path, while miners were still searching.
//!
//! # What is not here
//!
//! Availability. Narwhal answers "can I fetch this batch?" with a quorum
//! certificate from a known committee; this chain has no committee. A validator
//! that cannot fetch a referenced batch simply does not build on the block that
//! referenced it. That is a weaker guarantee and it is named in the crate
//! documentation rather than implied away.

use alloc::vec::Vec;

use crate::error::{GraphError, Result};

/// Identifier of a batch: the digest of its canonical encoding.
///
/// Opaque here. This crate cannot hash — see the crate documentation — so
/// identifiers are computed in `custom-l1-node` and arrive as bytes.
pub type BatchId = [u8; 32];

/// Batch references permitted in one block.
///
/// # Where the number comes from
///
/// It is a throughput target read backwards. With
/// [`MAX_BATCH_TRANSACTIONS`] at 512, 256 references admit 131,072
/// transactions per block, or about 8,738 per second at a 15-second target.
/// The reference list itself is then 8 KiB, which is small next to a header
/// that a miner is hashing anyway.
///
/// Raising it raises the ceiling linearly and raises the cost of the one thing
/// a validator cannot skip: it must fetch and verify every referenced batch
/// before it can check the state root, so this is also a bound on how much work
/// one block can demand of every node on the network.
///
/// [`MAX_BATCH_TRANSACTIONS`]: crate::batch::MAX_BATCH_TRANSACTIONS
pub const MAX_REFS_PER_BLOCK: usize = 256;

/// A block's batch references: strictly ascending, distinct, bounded.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BatchRefs {
    ids: Vec<BatchId>,
}

impl BatchRefs {
    /// Builds a reference set, enforcing order, distinctness, and the bound.
    ///
    /// # Why order is a rule and not a normalisation
    ///
    /// Sorting the input would mean one set of batches has many valid block
    /// encodings, and a miner can vary an encoding without redoing the work
    /// that produced the block — a nonce nobody charged for. The same reason
    /// `maya-lattice-pow` refuses an unreduced basis coefficient instead of
    /// reducing it.
    ///
    /// Strict ascent also makes distinctness free: a duplicate is a
    /// non-increase, so one pass decides both rules and there is no separate
    /// set to allocate.
    ///
    /// # Errors
    ///
    /// [`GraphError::TooManyRefs`] past [`MAX_REFS_PER_BLOCK`], or
    /// [`GraphError::RefsNotStrictlyAscending`] for an out-of-order or repeated
    /// identifier.
    pub fn new(ids: Vec<BatchId>) -> Result<Self> {
        if ids.len() > MAX_REFS_PER_BLOCK {
            return Err(GraphError::TooManyRefs);
        }
        if ids.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(GraphError::RefsNotStrictlyAscending);
        }
        Ok(Self { ids })
    }

    /// The identifiers, ascending.
    #[must_use]
    pub fn ids(&self) -> &[BatchId] {
        &self.ids
    }

    /// How many batches this block references.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// Whether this block references no batches.
    ///
    /// Legal: a block with no transactions is a block, and refusing one would
    /// stop a chain that had nothing to do.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// Whether `id` is referenced, in `O(log n)`.
    ///
    /// Binary search is sound precisely because the constructor enforced the
    /// ordering — the rule and the lookup are the same fact.
    #[must_use]
    pub fn contains(&self, id: &BatchId) -> bool {
        self.ids.binary_search(id).is_ok()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use alloc::vec;

    fn id(byte: u8) -> BatchId {
        [byte; 32]
    }

    #[test]
    fn an_empty_reference_set_is_legal() {
        let refs = BatchRefs::new(vec![]).expect("valid");
        assert!(refs.is_empty());
        assert_eq!(refs.len(), 0);
    }

    #[test]
    fn ascending_distinct_identifiers_are_accepted() {
        let refs = BatchRefs::new(vec![id(1), id(2), id(3)]).expect("valid");
        assert_eq!(refs.len(), 3);
        assert!(refs.contains(&id(2)));
        assert!(!refs.contains(&id(4)));
    }

    #[test]
    fn a_descending_pair_is_refused_rather_than_sorted() {
        assert_eq!(
            BatchRefs::new(vec![id(2), id(1)]),
            Err(GraphError::RefsNotStrictlyAscending)
        );
    }

    #[test]
    fn a_repeated_identifier_is_refused() {
        assert_eq!(
            BatchRefs::new(vec![id(1), id(1)]),
            Err(GraphError::RefsNotStrictlyAscending)
        );
    }

    #[test]
    fn a_set_at_the_limit_is_accepted_and_one_past_it_is_not() {
        let at_limit: Vec<BatchId> = (0..MAX_REFS_PER_BLOCK)
            .map(|i| {
                let mut value = [0u8; 32];
                value[0..8].copy_from_slice(&(i as u64).to_be_bytes());
                value
            })
            .collect();
        assert!(BatchRefs::new(at_limit.clone()).is_ok());

        let mut past_limit = at_limit;
        past_limit.push([0xFF; 32]);
        assert_eq!(BatchRefs::new(past_limit), Err(GraphError::TooManyRefs));
    }

    #[test]
    fn the_count_limit_is_reported_before_the_ordering_rule() {
        // A set breaking both must report the count, so two validators agree
        // on which rule a peer broke.
        let ids = vec![id(9); MAX_REFS_PER_BLOCK + 1];
        assert_eq!(BatchRefs::new(ids), Err(GraphError::TooManyRefs));
    }
}
