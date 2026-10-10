//! A batch: the unit that is disseminated ahead of the block that commits to
//! it.
//!
//! Transactions are held as opaque byte strings. This crate cannot decode a
//! `Maya2C` transaction — that would mean depending on `custom-l1-node` — and it
//! does not need to. What it enforces are the bounds that make a batch safe to
//! accept from a stranger: how many, how large, and none of them empty.

use alloc::vec::Vec;

use crate::error::{GraphError, Result};

/// Transactions permitted in one batch.
///
/// # Why 512 and not a round power of two further up
///
/// A `Maya2C` transaction carries a hybrid signature pair — ML-DSA-65 at 3,309
/// bytes and SLH-DSA-SHA2-128s at 7,856 — so 11,165 bytes before any payload.
/// 512 of them is about 5.7 MB, which is a batch a peer can hold, verify, and
/// discard without the count alone becoming a memory bound. 4,096 would be
/// 45 MB, and a handful of those in flight is a node's entire heap.
pub const MAX_BATCH_TRANSACTIONS: usize = 512;

/// Encoded bytes permitted in one batch: 8 MiB.
///
/// A second bound rather than a redundant one. 512 minimal transactions and 512
/// maximal ones differ by orders of magnitude in size, and it is bytes that
/// bound a peer's memory. The count limit bounds verification work; this bounds
/// what the network can make a node hold.
pub const MAX_BATCH_BYTES: usize = 8 * 1024 * 1024;

/// An ordered group of encoded transactions.
///
/// The order is preserved and is part of what the batch identifier commits to:
/// two batches with the same transactions in different orders are different
/// batches, because they execute differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    transactions: Vec<Vec<u8>>,
}

impl Batch {
    /// Builds a batch, enforcing every bound.
    ///
    /// # Errors
    ///
    /// [`GraphError::EmptyBatch`] for no transactions,
    /// [`GraphError::BatchTooManyTransactions`] past
    /// [`MAX_BATCH_TRANSACTIONS`], [`GraphError::EmptyTransaction`] for a
    /// zero-length entry, or [`GraphError::BatchTooLarge`] past
    /// [`MAX_BATCH_BYTES`].
    ///
    /// The checks run in that order, and the order is part of the rule: two
    /// validators reporting different errors for a batch that breaks more than
    /// one bound would disagree in any log or ban score keyed on the error.
    pub fn new(transactions: Vec<Vec<u8>>) -> Result<Self> {
        if transactions.is_empty() {
            return Err(GraphError::EmptyBatch);
        }
        if transactions.len() > MAX_BATCH_TRANSACTIONS {
            return Err(GraphError::BatchTooManyTransactions);
        }
        if transactions.iter().any(Vec::is_empty) {
            return Err(GraphError::EmptyTransaction);
        }

        // Summed with `checked_add` rather than `sum()`. The inputs arrive from
        // the network, and a total that wrapped would compare below the limit
        // and admit exactly the batch the limit exists to refuse.
        let mut total: usize = 0;
        for transaction in &transactions {
            total = total
                .checked_add(transaction.len())
                .ok_or(GraphError::BatchTooLarge)?;
            if total > MAX_BATCH_BYTES {
                return Err(GraphError::BatchTooLarge);
            }
        }

        Ok(Self { transactions })
    }

    /// The transactions, in the order they will execute.
    #[must_use]
    pub fn transactions(&self) -> &[Vec<u8>] {
        &self.transactions
    }

    /// How many transactions this batch carries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    /// Always `false` — a batch cannot be empty and still exist.
    ///
    /// Present because clippy asks for it alongside [`Batch::len`], and it is
    /// implemented honestly rather than by testing the vector: [`Batch::new`]
    /// rejects the empty case, so returning anything else would mean the
    /// constructor had been bypassed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Total encoded size of the transactions, in bytes.
    ///
    /// Cannot overflow: [`Batch::new`] proved the sum fits before constructing
    /// the value.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.transactions.iter().map(Vec::len).sum()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use alloc::vec;

    #[test]
    fn an_empty_batch_is_refused() {
        assert_eq!(Batch::new(vec![]), Err(GraphError::EmptyBatch));
    }

    #[test]
    fn a_zero_length_transaction_is_refused() {
        assert_eq!(
            Batch::new(vec![vec![1, 2, 3], vec![]]),
            Err(GraphError::EmptyTransaction)
        );
    }

    #[test]
    fn a_batch_at_the_count_limit_is_accepted_and_one_past_it_is_not() {
        let at_limit = vec![vec![1u8]; MAX_BATCH_TRANSACTIONS];
        assert!(Batch::new(at_limit).is_ok());

        let past_limit = vec![vec![1u8]; MAX_BATCH_TRANSACTIONS + 1];
        assert_eq!(
            Batch::new(past_limit),
            Err(GraphError::BatchTooManyTransactions)
        );
    }

    #[test]
    fn a_batch_past_the_byte_limit_is_refused() {
        // Two transactions, together one byte over the ceiling.
        let half = MAX_BATCH_BYTES / 2;
        let transactions = vec![vec![0u8; half], vec![0u8; half + 1]];
        assert_eq!(Batch::new(transactions), Err(GraphError::BatchTooLarge));
    }

    #[test]
    fn the_count_limit_is_reported_before_the_byte_limit() {
        // A batch breaking both must report the count, or two validators
        // disagree about which rule a peer broke.
        let transactions = vec![vec![0u8; 1024 * 1024]; MAX_BATCH_TRANSACTIONS + 1];
        assert_eq!(
            Batch::new(transactions),
            Err(GraphError::BatchTooManyTransactions)
        );
    }

    #[test]
    fn a_batch_preserves_the_order_it_was_given() {
        let batch = Batch::new(vec![vec![3], vec![1], vec![2]]).expect("valid");
        assert_eq!(batch.transactions(), &[vec![3], vec![1], vec![2]]);
        assert_eq!(batch.len(), 3);
        assert_eq!(batch.byte_len(), 3);
    }
}
