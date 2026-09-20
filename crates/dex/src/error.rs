//! Failure modes of the trading engine.
//!
//! These are *arithmetic and rule* failures, not chain errors. The crate
//! returns its own type for the same reason [`maya_ledger_math`] returns
//! `Option`: keeping `custom-l1-node` out of the dependency graph is the point
//! of the crate boundary, and the mapping from "this trade cannot be
//! represented" to whichever `NodeError` variant a context calls for belongs to
//! the caller.
//!
//! [`maya_ledger_math`]: https://docs.rs/maya-ledger-math

use core::fmt;

/// Why a trading operation could not be performed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DexError {
    /// An intermediate or result exceeded `u64`.
    ///
    /// Distinct from a rule violation: it means the trade is unrepresentable at
    /// this pool's size, not that it was disallowed.
    Overflow,
    /// A trade or deposit named a zero amount.
    ///
    /// Rejected rather than treated as a no-op, so a zero-amount order can
    /// never occupy a slot in the book or a line in a batch.
    ZeroAmount,
    /// The pool holds no liquidity on at least one side.
    EmptyPool,
    /// The requested output is at least the whole of the output reserve.
    ///
    /// A constant-product pool cannot be emptied — the price goes to infinity
    /// first — so this is a rule, not a bound that a larger input could clear.
    InsufficientLiquidity,
    /// A fee rate above [`crate::fees::MAX_TOTAL_FEE_BPS`].
    FeeTooHigh,
    /// The first deposit into a pool was too small to mint a share after the
    /// permanently locked minimum was withheld.
    InsufficientInitialLiquidity,
    /// A share count exceeding the pool's issued supply was presented for
    /// redemption.
    InsufficientShares,
    /// A redemption or deposit would round to zero on at least one side.
    ///
    /// Rejected rather than silently paying nothing: a caller that burns shares
    /// and receives no asset has been robbed by a rounding rule.
    ZeroOutput,
    /// The constant-product invariant would have decreased.
    ///
    /// Unreachable by construction — every path here is proved to hold it — so
    /// reaching it means a bug in this crate, and the correct response is to
    /// abandon the trade rather than to write the result.
    InvariantViolation,
    /// A price of zero, which no order or pool may quote.
    ZeroPrice,
    /// More work was requested than the caller's bound allows.
    ///
    /// Not a failure of the trade: the caller sets a ceiling on matching work
    /// so that one transaction cannot make every node on the network do
    /// unbounded work, and this reports that the ceiling was reached.
    WorkLimitReached,
}

impl fmt::Display for DexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Overflow => "trade arithmetic exceeded 64 bits",
            Self::ZeroAmount => "amount must be non-zero",
            Self::EmptyPool => "pool holds no liquidity",
            Self::InsufficientLiquidity => "output exceeds available reserve",
            Self::FeeTooHigh => "fee rate above the permitted maximum",
            Self::InsufficientInitialLiquidity => {
                "initial deposit too small to mint a share after the locked minimum"
            }
            Self::InsufficientShares => "share count exceeds issued supply",
            Self::ZeroOutput => "operation would return zero on at least one side",
            Self::InvariantViolation => "constant-product invariant would decrease",
            Self::ZeroPrice => "price must be non-zero",
            Self::WorkLimitReached => "matching work limit reached",
        };
        f.write_str(message)
    }
}

impl core::error::Error for DexError {}

/// Convenience alias for this crate's fallible operations.
pub type Result<T> = core::result::Result<T, DexError>;
