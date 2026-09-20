//! The neural rule: EIP-1559's step, scaled by a gain that can only ever move
//! the fee in the direction that costs the block producer.
//!
//! # The envelope
//!
//! ```text
//! linear = parent · |size − target| / target / denominator      (base_fee.rs)
//! over target:   step = linear · clamp(gain, 1.0, 2.0)   next = parent + max(step, 1)
//! under target:  step = linear · clamp(gain, 0.0, 1.0)   next = parent − step
//! at target:     next = parent
//! next = max(next, floor)
//! ```
//!
//! For every gain, and so for every weight file:
//!
//! 1. **Never below EIP-1559.** `next ≥ next_base_fee(...)` on the same inputs.
//! 2. **Direction.** Over target the fee never falls; under target it never
//!    rises (except to meet the floor); at target it holds.
//! 3. **Speed.** A rise is at most twice EIP-1559's; a fall at most EIP-1559's.
//! 4. **Floor.** Never below `floor`.
//!
//! `src/proofs.rs` proves all four under Kani.
//!
//! # Why the gain is one-sided
//!
//! The first version let the gain range over `[0, 2]` in both directions and
//! capped every move at one `1/denominator` step. Review on 2026-09-14 showed
//! what that allowed. 80% of the base fee burns, so a producer gains from a
//! lower one, and a producer writes the features. Shaping a full block's
//! features to drive the gain to zero turned EIP-1559's forced one-eighth rise
//! into a rise of one unit per block: from a base fee of 1,000, about 125 blocks
//! instead of about 8 to price a burst. The fee never fell, but it could be held
//! down.
//!
//! A producer wants the fee to rise slowly and fall fast, so the network may
//! only make rises faster and falls slower. Whatever features a producer
//! writes, the result is a fee at least as high as EIP-1559's — the lever points
//! the wrong way for them. What that gives up: the network cannot cut the fee
//! faster than EIP-1559 when demand collapses. A rival producer pushing fees
//! *up* is bounded by the doubled rise.

use crate::model::{Features, MAX_GAIN_BPS, Model, UNIT_GAIN_BPS};
use crate::next_base_fee;

/// Which base-fee rule a block uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeeRule {
    /// EIP-1559's linear step.
    Linear,
    /// The linear step scaled by a compiled-in network's gain.
    Neural(&'static Model),
}

/// The next base fee under a gain, in basis points.
///
/// Arithmetic is in `u128` with saturating multiplication, and the result
/// saturates at `u64::MAX`, so no input panics. `target` and `denominator` are
/// assumed validated, as for [`next_base_fee`]; a zero means "no change".
#[must_use]
pub fn neural_next_base_fee(
    parent_base_fee: u64,
    parent_size: u64,
    target: u64,
    denominator: u64,
    floor: u64,
    gain_bps: u64,
) -> u64 {
    if target == 0 || denominator == 0 || parent_size == target {
        return parent_base_fee.max(floor);
    }
    let base = u128::from(parent_base_fee);
    let rising = parent_size > target;
    let gap = u128::from(parent_size.abs_diff(target));
    let linear = base * gap / u128::from(target) / u128::from(denominator);
    let gain = if rising {
        gain_bps.clamp(UNIT_GAIN_BPS, MAX_GAIN_BPS)
    } else {
        gain_bps.min(UNIT_GAIN_BPS)
    };
    let scaled = linear.saturating_mul(u128::from(gain)) / u128::from(UNIT_GAIN_BPS);
    // Bounded against the linear step itself, not only through the gain: a
    // saturated product divided back down can land below `linear`, and a rise
    // below EIP-1559's is exactly what this rule exists to rule out.
    let step = if rising {
        scaled.max(linear)
    } else {
        scaled.min(linear)
    };

    let next = if rising {
        base.saturating_add(step.max(1))
    } else {
        base.saturating_sub(step)
    };
    u64::try_from(next).unwrap_or(u64::MAX).max(floor)
}

/// The next base fee under a rule.
///
/// `features` describe the parent block and are read only by
/// [`FeeRule::Neural`].
#[must_use]
pub fn next_base_fee_by_rule(
    rule: FeeRule,
    parent_base_fee: u64,
    parent_size: u64,
    target: u64,
    denominator: u64,
    floor: u64,
    features: &Features,
) -> u64 {
    match rule {
        FeeRule::Linear => next_base_fee(parent_base_fee, parent_size, target, denominator, floor),
        FeeRule::Neural(model) => neural_next_base_fee(
            parent_base_fee,
            parent_size,
            target,
            denominator,
            floor,
            model.gain(features),
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const TARGET: u64 = 1_000_000;
    const DENOM: u64 = 8;

    const SIZES: [u64; 9] = [
        0, 250_000, 999_999, 1_000_000, 1_000_001, 1_500_000, 2_000_000, 4_000_000, 8_000_000,
    ];
    const PARENTS: [u64; 5] = [1, 7, 100, 1_000_000, u64::MAX / 2];

    #[test]
    fn unit_gain_is_the_linear_rule_everywhere() {
        for size in SIZES {
            for parent in PARENTS {
                assert_eq!(
                    neural_next_base_fee(parent, size, TARGET, DENOM, 1, UNIT_GAIN_BPS),
                    next_base_fee(parent, size, TARGET, DENOM, 1),
                    "parent {parent} size {size}"
                );
            }
        }
    }

    #[test]
    fn no_gain_ever_prices_below_the_linear_rule() {
        for size in SIZES {
            for parent in PARENTS {
                for gain in [0, 1, 5_000, UNIT_GAIN_BPS, 15_000, MAX_GAIN_BPS, u64::MAX] {
                    let neural = neural_next_base_fee(parent, size, TARGET, DENOM, 1, gain);
                    let linear = next_base_fee(parent, size, TARGET, DENOM, 1);
                    assert!(neural >= linear, "parent {parent} size {size} gain {gain}");
                }
            }
        }
    }

    #[test]
    fn a_zero_gain_cannot_slow_a_rise_it_can_only_hold_a_fall() {
        // Over target: clamped up to the linear step, never down to one unit.
        assert_eq!(
            neural_next_base_fee(1_000, 1_800_000, TARGET, DENOM, 1, 0),
            next_base_fee(1_000, 1_800_000, TARGET, DENOM, 1)
        );
        // Under target: the fee holds.
        assert_eq!(neural_next_base_fee(1_000, 0, TARGET, DENOM, 1, 0), 1_000);
    }

    #[test]
    fn the_largest_gain_doubles_a_rise_and_no_more() {
        // Half over target: the linear step is 8,000 / 16 = 500.
        assert_eq!(
            neural_next_base_fee(8_000, 1_500_000, TARGET, DENOM, 1, MAX_GAIN_BPS),
            9_000
        );
        assert_eq!(
            neural_next_base_fee(8_000, 1_500_000, TARGET, DENOM, 1, u64::MAX),
            9_000
        );
    }

    #[test]
    fn a_gain_above_one_cannot_speed_a_fall() {
        assert_eq!(
            neural_next_base_fee(8_000, 500_000, TARGET, DENOM, 1, MAX_GAIN_BPS),
            next_base_fee(8_000, 500_000, TARGET, DENOM, 1)
        );
    }

    #[test]
    fn extreme_inputs_saturate_instead_of_overflowing() {
        assert_eq!(
            neural_next_base_fee(u64::MAX, u64::MAX, 1, 8, 1, MAX_GAIN_BPS),
            u64::MAX
        );
    }

    #[test]
    fn the_rule_enum_dispatches() {
        let features = Features::new([0; crate::model::INPUTS]);
        assert_eq!(
            next_base_fee_by_rule(FeeRule::Linear, 100, 2_000_000, TARGET, DENOM, 1, &features),
            next_base_fee(100, 2_000_000, TARGET, DENOM, 1)
        );
        assert_eq!(
            next_base_fee_by_rule(
                FeeRule::Neural(&Model::ZERO),
                100,
                500_000,
                TARGET,
                DENOM,
                1,
                &features
            ),
            100
        );
    }
}
