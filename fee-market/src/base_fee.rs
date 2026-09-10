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
//! next  = parent_base_fee + max(delta, 1)   if parent_size > target
//!         parent_base_fee − delta           if parent_size < target
//!         parent_base_fee                   otherwise
//! next  = max(next, floor)
//! ```
//!
//! The `max(delta, 1)` on the way up is EIP-1559's, and it matters: without it
//! a base fee of 7 over a denominator of 8 rounds every increase to zero, and a
//! chain at the floor can be kept full forever at the floor price.

/// The next block's base fee, per byte.
///
/// Arithmetic is in `u128` and the result saturates at `u64::MAX`, so no input
/// panics and none wraps. `target` and `denominator` are assumed validated —
/// [`crate::FeeConfig::validate`] guarantees both are non-zero — but a zero is
/// treated as "no change" rather than a division by zero, so a caller that
/// skipped validation gets a constant fee and not a crash.
#[must_use]
pub fn next_base_fee(
    parent_base_fee: u64,
    parent_size: u64,
    target: u64,
    denominator: u64,
    floor: u64,
) -> u64 {
    if target == 0 || denominator == 0 {
        return parent_base_fee.max(floor);
    }

    let base = u128::from(parent_base_fee);
    let (gap, rising) = if parent_size >= target {
        (u128::from(parent_size - target), parent_size > target)
    } else {
        (u128::from(target - parent_size), false)
    };
    let delta = base * gap / u128::from(target) / u128::from(denominator);

    let next = if rising {
        base.saturating_add(delta.max(1))
    } else {
        base.saturating_sub(delta)
    };
    u64::try_from(next).unwrap_or(u64::MAX).max(floor)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: u64 = 1_000_000;
    const DENOM: u64 = 8;
    const FLOOR: u64 = 1;

    #[test]
    fn a_block_at_target_leaves_the_fee_unchanged() {
        assert_eq!(next_base_fee(1_000, TARGET, TARGET, DENOM, FLOOR), 1_000);
    }

    #[test]
    fn a_full_block_raises_the_fee_by_exactly_one_eighth() {
        // Twice the target is a gap of one target: delta = fee / 8.
        assert_eq!(
            next_base_fee(8_000, 2 * TARGET, TARGET, DENOM, FLOOR),
            9_000
        );
    }

    #[test]
    fn an_empty_block_lowers_the_fee_by_exactly_one_eighth() {
        assert_eq!(next_base_fee(8_000, 0, TARGET, DENOM, FLOOR), 7_000);
    }

    #[test]
    fn the_fee_never_moves_faster_than_the_denominator_allows() {
        // However far over target a block is -- a malformed size, a future
        // larger block limit -- the step up is bounded only by the gap. Pin
        // that the empty-block step down, the largest *possible* decrease, is
        // exactly one denominator's worth and never more.
        for fee in [8u64, 1_000, 123_456, u64::MAX / 16] {
            let down = next_base_fee(fee, 0, TARGET, DENOM, 0);
            assert_eq!(fee - down, fee / DENOM, "fee {fee}");
        }
    }

    #[test]
    fn a_small_fee_still_rises_when_blocks_are_full() {
        // 7 · 1 / 8 rounds to zero. Without the max(delta, 1) the fee at 7
        // would never move, and a full chain would stay at 7 forever.
        assert_eq!(next_base_fee(7, 2 * TARGET, TARGET, DENOM, FLOOR), 8);
    }

    #[test]
    fn the_floor_holds_under_a_run_of_empty_blocks() {
        let mut fee = 1_000_000;
        for _ in 0..1_000 {
            fee = next_base_fee(fee, 0, TARGET, DENOM, 50);
        }
        assert_eq!(fee, 50);
    }

    #[test]
    fn sustained_demand_converges_upward_and_its_absence_back_down() {
        let mut fee = 100;
        for _ in 0..50 {
            fee = next_base_fee(fee, 2 * TARGET, TARGET, DENOM, FLOOR);
        }
        let peak = fee;
        assert!(peak > 100 * 100, "fifty full blocks: {peak}");
        for _ in 0..200 {
            fee = next_base_fee(fee, 0, TARGET, DENOM, FLOOR);
        }
        assert!(fee < 100, "two hundred empty blocks: {fee}");
    }

    #[test]
    fn extreme_inputs_saturate_rather_than_panic_or_wrap() {
        assert_eq!(next_base_fee(u64::MAX, u64::MAX, 1, DENOM, FLOOR), u64::MAX);
        assert_eq!(next_base_fee(0, u64::MAX, 1, DENOM, FLOOR), 1);
        assert_eq!(
            next_base_fee(5, 10, 0, DENOM, FLOOR),
            5,
            "zero target is a constant"
        );
        assert_eq!(
            next_base_fee(5, 10, TARGET, 0, FLOOR),
            5,
            "zero denominator is a constant"
        );
    }
}
