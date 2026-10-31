//! The EIP-1559 control law, measured in bytes.
//!
//! # Why bytes and not gas
//!
//! There is no block gas here. Gas exists only as wasmtime fuel for contract
//! calls, and it is a halting bound rather than a price; transfers, swaps and
//! votes consume none. What every transaction does consume is **space**: a
//! hybrid signature is 11,165 bytes, so a block fills by size long before any
//! plausible compute budget. The scarce resource is bytes, so the fee tracks
//! bytes, and the base fee is quoted per serialized byte.
//!
//! # The law
//!
//! ```text
//! delta = parent_base_fee · |parent_size − target| / target / denominator
//! rise  = min(max(delta, 1), max(parent_base_fee / denominator, 1))
//! next  = min(parent_base_fee + rise, max(MAX_BASE_FEE_PER_BYTE, parent_base_fee))
//!                                           if parent_size > target
//!         parent_base_fee − delta           if parent_size < target
//!         parent_base_fee                   otherwise
//! next  = max(next, floor)
//! ```
//!
//! The `max(delta, 1)` on the way up is EIP-1559's, and it matters: without it
//! a base fee of 7 over a denominator of 8 rounds every increase to zero, and a
//! chain at the floor can be kept full forever at the floor price. The
//! `min(…, parent / denominator)` is the other half: without it a block at 64
//! times the target multiplies the fee by about 8.9 (ADR-071). The fall needs
//! no such bound, since `parent_size ≥ 0` keeps the gap at most `target`.

use maya_amount::Amount;

use crate::limits::MAX_BASE_FEE_PER_BYTE;

/// The next block's base fee, per byte.
///
/// Fees are 256-bit [`Amount`]s; the product is taken in 512 bits and the result
/// saturates at `Amount::MAX`, so no input panics and none wraps. `target` and `denominator` are assumed validated —
/// [`crate::FeeConfig::validate`] guarantees both are non-zero — but a zero is
/// treated as "no change" rather than a division by zero, so a caller that
/// skipped validation gets a constant fee and not a crash.
#[must_use]
pub fn next_base_fee(
    parent_base_fee: Amount,
    parent_size: u64,
    target: u64,
    denominator: u64,
    floor: Amount,
) -> Amount {
    if target == 0 || denominator == 0 {
        return parent_base_fee.max(floor);
    }

    let (gap, rising) = if parent_size >= target {
        (parent_size - target, parent_size > target)
    } else {
        (target - parent_size, false)
    };
    // `base * gap / target / denominator` is `base * gap / (target * denominator)`:
    // nested floors agree, and the product cannot overflow inside `mul_div`.
    let scale = Amount::from_u128(u128::from(target) * u128::from(denominator));
    let delta = parent_base_fee
        .mul_div(Amount::from_u64(gap), scale)
        .unwrap_or(Amount::MAX);

    let next = if rising {
        // One denominator's share of the fee at most, never less than one unit.
        let share = parent_base_fee
            .checked_div(Amount::from_u64(denominator))
            .unwrap_or(Amount::ZERO)
            .max(Amount::from_u64(1));
        let rise = delta.max(Amount::from_u64(1)).min(share);
        // A fee already above the ceiling holds; it does not fall to meet it.
        parent_base_fee
            .saturating_add(rise)
            .min(MAX_BASE_FEE_PER_BYTE.max(parent_base_fee))
    } else {
        parent_base_fee.saturating_sub(delta)
    };
    next.max(floor)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const TARGET: u64 = 1_000_000;
    const DENOM: u64 = 8;

    fn a(n: u64) -> Amount {
        Amount::from_u64(n)
    }

    /// One step with the module's target and denominator.
    fn next(fee: Amount, size: u64, floor: u64) -> Amount {
        next_base_fee(fee, size, TARGET, DENOM, a(floor))
    }

    #[test]
    fn a_block_at_target_leaves_the_fee_unchanged() {
        assert_eq!(next(a(1_000), TARGET, 1), a(1_000));
    }

    #[test]
    fn a_full_block_raises_the_fee_by_exactly_one_eighth() {
        // Twice the target is a gap of one target: delta = fee / 8.
        assert_eq!(next(a(8_000), 2 * TARGET, 1), a(9_000));
    }

    #[test]
    fn an_empty_block_lowers_the_fee_by_exactly_one_eighth() {
        assert_eq!(next(a(8_000), 0, 1), a(7_000));
    }

    #[test]
    fn the_fee_never_moves_faster_than_the_denominator_allows() {
        // However far over target a block is -- a malformed size, a future
        // larger block limit -- the step up is bounded only by the gap. Pin
        // that the empty-block step down, the largest *possible* decrease, is
        // exactly one denominator's worth and never more.
        let big = Amount::MAX.shr(4);
        for fee in [a(8), a(1_000), a(123_456), big] {
            let down = next(fee, 0, 0);
            let eighth = fee.checked_div(a(DENOM)).unwrap();
            assert_eq!(fee.saturating_sub(down), eighth, "fee {fee}");
        }
    }

    #[test]
    fn a_small_fee_still_rises_when_blocks_are_full() {
        // 7 * 1 / 8 rounds to zero. Without the max(delta, 1) the fee at 7
        // would never move, and a full chain would stay at 7 forever.
        assert_eq!(next(a(7), 2 * TARGET, 1), a(8));
    }

    #[test]
    fn the_floor_holds_under_a_run_of_empty_blocks() {
        let mut fee = a(1_000_000);
        for _ in 0..1_000 {
            fee = next(fee, 0, 50);
        }
        assert_eq!(fee, a(50));
    }

    #[test]
    fn sustained_demand_converges_upward_and_its_absence_back_down() {
        let mut fee = a(100);
        for _ in 0..50 {
            fee = next(fee, 2 * TARGET, 1);
        }
        let peak = fee;
        assert!(peak > a(100 * 100), "fifty full blocks: {peak}");
        for _ in 0..200 {
            fee = next(fee, 0, 1);
        }
        assert!(fee < a(100), "two hundred empty blocks: {fee}");
    }

    #[test]
    fn a_fee_below_the_ceiling_rises_by_exactly_one_eighth() {
        // Well past u64 but under the ceiling: the step is exact, not clamped.
        let fee = Amount::from_u64(4_000_000_000_000_000_000);
        let expected = fee.saturating_add(fee.checked_div(a(DENOM)).unwrap());
        assert_eq!(next(fee, 2 * TARGET, 1), expected);
    }

    #[test]
    fn a_fee_above_the_ceiling_holds_on_a_full_block() {
        // 2^200 is above MAX_BASE_FEE_PER_BYTE: a rise must not lift it further.
        let fee = Amount([0, 0, 0, 1 << 8]);
        assert_eq!(next(fee, 2 * TARGET, 1), fee);
    }

    #[test]
    fn extreme_inputs_saturate_rather_than_panic_or_wrap() {
        // Saturates at the 256-bit ceiling, not at u64::MAX.
        assert_eq!(
            next_base_fee(Amount::MAX, u64::MAX, 1, DENOM, a(1)),
            Amount::MAX
        );
        assert_eq!(next_base_fee(a(0), u64::MAX, 1, DENOM, a(1)), a(1));
        assert_eq!(
            next_base_fee(a(5), 10, 0, DENOM, a(1)),
            a(5),
            "zero target is a constant"
        );
        assert_eq!(
            next_base_fee(a(5), 10, TARGET, 0, a(1)),
            a(5),
            "zero denominator is a constant"
        );
    }
}
