//! Execution context for a block.

/// Chain facts a transaction may depend on while executing.
///
/// A newtype rather than a bare `u64` parameter: `apply_block(&block, 7)` next
/// to a nonce or an amount is easy to get wrong and compiles silently either
/// way. Timelocks are the reason this exists at all — an HTLC expiry or a
/// dispute deadline is meaningless without knowing which block is executing.
///
/// Height rather than timestamp: block timestamps are miner-influenced within
/// the consensus tolerance, so a timestamp-based deadline is a deadline an
/// adversary can nudge. Height cannot be forged without doing the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockContext {
    /// Height of the block being executed.
    pub height: u64,
}

impl BlockContext {
    /// Context for a block at `height`.
    #[must_use]
    pub const fn at_height(height: u64) -> Self {
        Self { height }
    }

    /// Genesis-era context.
    ///
    /// Correct for plain transfers, which do not consult the height at all.
    /// Anything with a timelock must use the real height.
    pub const GENESIS: Self = Self { height: 0 };
}

impl Default for BlockContext {
    fn default() -> Self {
        Self::GENESIS
    }
}
