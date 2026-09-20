//! The fee split: what the liquidity providers keep and what the chain takes.
//!
//! # Where an LP fee actually lives
//!
//! It is never paid out. The fee stays in the reserves, so `k` grows and every
//! outstanding share redeems for slightly more than it did before. There is no
//! accrual counter, no per-provider accumulator, and no claim transaction.
//!
//! The alternative — a per-share fee accumulator, incremented on every swap and
//! settled when a provider withdraws — is what a protocol reaches for when fees
//! are paid in an asset the pool does not hold. Here they are paid in the asset
//! the pool does hold, so the accumulator would be a second representation of a
//! number the reserves already carry, kept in step by hand, and drifting by a
//! unit every time a division was inexact.
//!
//! The consequence to be aware of: a provider's share count never changes, so
//! "how much have I earned" is `shares * reserves / supply` now minus what it
//! was at deposit, and nothing on chain records the second term. That is a
//! wallet's job, not the ledger's.
//!
//! # Why the protocol cut comes off the input
//!
//! The protocol's share is taken from `amount_in` *before* the curve sees it,
//! not skimmed from the pool afterwards. Skimming afterwards would mean
//! withdrawing from the reserves, which lowers `k` — and then "did this swap
//! decrease `k`" stops being a usable invariant, because the honest answer
//! becomes "yes, by exactly the amount we intended". An invariant with an
//! exception is not an invariant.

use crate::error::{DexError, Result};

/// Denominator for every rate in this crate: rates are basis points.
pub const FEE_DENOMINATOR: u32 = 10_000;

/// Ceiling on the combined LP and protocol rate.
///
/// Five percent. Not a market judgement — pools set their own rate below it —
/// but a bound on how badly a pool creator can misconfigure one, and on how
/// much a governance change could take without the pool being redeployed.
pub const MAX_TOTAL_FEE_BPS: u32 = 500;

/// The default a pool gets when its creator does not choose.
///
/// Thirty basis points, matching the constant-product convention. Chosen for
/// familiarity rather than derived: there is no analysis here that says 30 is
/// right for this chain, and pretending otherwise would be worse than saying
/// so.
pub const DEFAULT_LP_FEE_BPS: u32 = 30;

/// How a pool splits the fee on a swap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FeeSchedule {
    /// Basis points retained in the reserves for liquidity providers.
    pub lp_bps: u32,
    /// Basis points diverted to the protocol fee sink.
    ///
    /// Zero on every pool until governance sets otherwise. A pool created with
    /// a non-zero protocol cut is a pool whose providers agreed to it.
    pub protocol_bps: u32,
}

impl FeeSchedule {
    /// A schedule with the conventional LP rate and no protocol cut.
    #[must_use]
    pub const fn standard() -> Self {
        Self {
            lp_bps: DEFAULT_LP_FEE_BPS,
            protocol_bps: 0,
        }
    }

    /// Builds a schedule, rejecting a combined rate above
    /// [`MAX_TOTAL_FEE_BPS`].
    ///
    /// # Errors
    ///
    /// Returns [`DexError::FeeTooHigh`] if the two rates together exceed the
    /// ceiling, or if either alone overflows the sum.
    pub const fn new(lp_bps: u32, protocol_bps: u32) -> Result<Self> {
        let Some(total) = lp_bps.checked_add(protocol_bps) else {
            return Err(DexError::FeeTooHigh);
        };
        if total > MAX_TOTAL_FEE_BPS {
            return Err(DexError::FeeTooHigh);
        }
        Ok(Self {
            lp_bps,
            protocol_bps,
        })
    }

    /// Combined rate in basis points.
    #[must_use]
    pub const fn total_bps(&self) -> u32 {
        // Cannot overflow: `new` is the only constructor that admits non-zero
        // values and it bounds the sum by `MAX_TOTAL_FEE_BPS`.
        self.lp_bps + self.protocol_bps
    }

    /// Whether the schedule is within bounds.
    ///
    /// Checked again at use rather than trusted from construction, because a
    /// schedule can also arrive by being decoded from a stored pool record,
    /// which no constructor guarded.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        match self.lp_bps.checked_add(self.protocol_bps) {
            Some(total) => total <= MAX_TOTAL_FEE_BPS,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn standard_schedule_takes_nothing_for_the_protocol() {
        let schedule = FeeSchedule::standard();
        assert_eq!(schedule.lp_bps, DEFAULT_LP_FEE_BPS);
        assert_eq!(schedule.protocol_bps, 0);
        assert!(schedule.is_valid());
    }

    #[test]
    fn a_combined_rate_above_the_ceiling_is_rejected() {
        assert_eq!(
            FeeSchedule::new(MAX_TOTAL_FEE_BPS, 1),
            Err(DexError::FeeTooHigh)
        );
    }

    #[test]
    fn a_decoded_schedule_is_revalidated_rather_than_trusted() {
        let forged = FeeSchedule {
            lp_bps: 9_000,
            protocol_bps: 0,
        };
        assert!(!forged.is_valid());
    }
}
