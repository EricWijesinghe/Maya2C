//! Why a batch, a reference set, or a schedule was rejected.
//!
//! Rule failures, not chain errors. The crate returns its own type for the same
//! reason [`maya_dex`] does: keeping `custom-l1-node` out of the dependency
//! graph is the point of the crate boundary.
//!
//! Each variant names one distinguishable way an input can be wrong. They stay
//! separate rather than collapsing into `Invalid` because a validator that
//! cannot say which rule a block broke cannot be debugged, and because the tests
//! assert on the specific variant — a test checking only `is_err()` passes when
//! the wrong rule fired.
//!
//! [`maya_dex`]: https://docs.rs/maya-dex

use core::fmt;

/// Why a batch, reference set, or schedule is not admissible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphError {
    /// A batch carrying no transactions.
    ///
    /// Rejected rather than treated as a no-op: an empty batch still costs a
    /// digest in every block that references it and a round trip for every peer
    /// that fetches it, so it is free bandwidth for whoever sends it.
    EmptyBatch,
    /// More than [`MAX_BATCH_TRANSACTIONS`] transactions in one batch.
    ///
    /// [`MAX_BATCH_TRANSACTIONS`]: crate::batch::MAX_BATCH_TRANSACTIONS
    BatchTooManyTransactions,
    /// A batch whose encoded transactions exceed [`MAX_BATCH_BYTES`].
    ///
    /// Separate from the count limit: 512 minimal transactions and 512 maximal
    /// ones differ by two orders of magnitude in bytes, and it is bytes that
    /// bound a peer's memory.
    ///
    /// [`MAX_BATCH_BYTES`]: crate::batch::MAX_BATCH_BYTES
    BatchTooLarge,
    /// A transaction of zero length inside a batch.
    EmptyTransaction,
    /// More than [`MAX_REFS_PER_BLOCK`] batch references in one block.
    ///
    /// [`MAX_REFS_PER_BLOCK`]: crate::refs::MAX_REFS_PER_BLOCK
    TooManyRefs,
    /// Batch references that are not strictly ascending.
    ///
    /// Covers both an out-of-order identifier and a repeated one: a repeat is a
    /// non-increase. Rejected rather than sorted, because sorting silently would
    /// give one set of batches many block encodings.
    RefsNotStrictlyAscending,
    /// A transaction touching no shard at all.
    ///
    /// Every transaction reads or writes some account, so an empty access set
    /// means the caller failed to compute one — and a transaction with no
    /// declared accesses would be scheduled into every wave as conflict-free,
    /// which is the one way this scheduler can produce a wrong answer.
    EmptyAccessSet,
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyBatch => "a batch must carry at least one transaction",
            Self::BatchTooManyTransactions => "batch exceeds the transaction count limit",
            Self::BatchTooLarge => "batch exceeds the byte limit",
            Self::EmptyTransaction => "a batch may not carry a zero-length transaction",
            Self::TooManyRefs => "block exceeds the batch reference limit",
            Self::RefsNotStrictlyAscending => {
                "batch references must be strictly ascending and distinct"
            }
            Self::EmptyAccessSet => "a transaction must declare at least one shard access",
        };
        f.write_str(message)
    }
}

impl core::error::Error for GraphError {}

/// Convenience alias for this crate's fallible operations.
pub type Result<T> = core::result::Result<T, GraphError>;
