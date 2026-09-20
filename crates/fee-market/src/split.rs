//! Where a paid fee goes: burned, treasury, or the block's producer.
//!
//! # No unit is created or lost, and the remainder favours holders
//!
//! Splitting a base fee 80/20 in integers leaves a remainder whenever the fee is
//! not a multiple of five. A split that dropped it would destroy value; one that
//! rounded the treasury share up would create it. Here the treasury share is
//! rounded **down** and everything else is burned, so
//! `burned + treasury + tip == base_fee_paid + tip` holds exactly, and the only
//! direction a rounding error can ever move value is out of circulation.

use crate::limits::BPS;

/// One transaction's fee, divided.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeeSplit {
    /// Sent to the unspendable fee sink: leaves circulation for good.
    pub burned: u64,
    /// Credited to the treasury.
    pub treasury: u64,
    /// Credited to whoever produced the block.
    pub tip: u64,
}

impl FeeSplit {
    /// Everything the transaction paid.
    ///
    /// `u128` because three `u64` totals can exceed `u64`; the caller compares
    /// it against what was charged, and a sum that wrapped would pass that
    /// comparison by accident.
    #[must_use]
    pub fn total(self) -> u128 {
        u128::from(self.burned) + u128::from(self.treasury) + u128::from(self.tip)
    }
}

/// Splits a base fee between burn and treasury, and passes the tip through.
///
/// `treasury_bps` is the treasury's share of the base fee in basis points;
/// [`crate::FeeConfig::validate`] bounds it by
/// [`crate::limits::MAX_TREASURY_BPS`]. A value above 100% is clamped to 100%
/// rather than creating value.
#[must_use]
pub fn split(base_fee_paid: u64, tip: u64, treasury_bps: u64) -> FeeSplit {
    let bps = u128::from(treasury_bps.min(BPS));
    let treasury = u64::try_from(u128::from(base_fee_paid) * bps / u128::from(BPS))
        .expect("a share of at most 100% of a u64 fits in a u64");
    FeeSplit {
        burned: base_fee_paid - treasury,
        treasury,
        tip,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const EIGHTY_TWENTY: u64 = 2_000;

    #[test]
    fn eighty_percent_is_burned_and_twenty_goes_to_the_treasury() {
        let s = split(1_000, 0, EIGHTY_TWENTY);
        assert_eq!((s.burned, s.treasury), (800, 200));
    }

    #[test]
    fn the_whole_tip_goes_to_the_producer() {
        assert_eq!(split(1_000, 37, EIGHTY_TWENTY).tip, 37);
    }

    #[test]
    fn the_remainder_is_burned_never_credited() {
        // 7 · 20% = 1.4: the treasury gets 1, the burn gets 6.
        let s = split(7, 0, EIGHTY_TWENTY);
        assert_eq!((s.burned, s.treasury), (6, 1));
    }

    #[test]
    fn no_unit_is_created_or_lost_across_a_wide_sweep() {
        // Exhaustive over small fees, where rounding is most of the value, and
        // sampled across the rest of the range.
        let samples = (0..=10_000u64)
            .chain((0..64).map(|shift| 1u64 << shift))
            .chain([u64::MAX, u64::MAX - 1, u64::MAX / 3]);
        for fee in samples {
            for bps in [0, 1, 1_999, EIGHTY_TWENTY, 5_000, BPS] {
                let s = split(fee, 11, bps);
                assert_eq!(s.total(), u128::from(fee) + 11, "fee {fee} at {bps} bps");
                assert!(s.treasury <= fee);
            }
        }
    }

    #[test]
    fn a_treasury_share_above_one_hundred_percent_cannot_create_value() {
        let s = split(1_000, 0, 50_000);
        assert_eq!((s.burned, s.treasury), (0, 1_000));
    }
}
