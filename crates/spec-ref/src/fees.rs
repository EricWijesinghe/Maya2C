//! Fee rules — `spec/04-fees.md` FEE-1 .. FEE-7.

use crate::balance::Balance;

/// Basis points in 100 %.
pub const BPS: u64 = 10_000;

/// FEE-6's ceiling, per byte: mirrors `maya_fee_market::limits::MAX_BASE_FEE_PER_BYTE`
/// (ADR-076, a placeholder pending the economics decision). Changing one
/// without the other splits the reference from the node.
pub const MAX_BASE_FEE_PER_BYTE: u64 = 7_567_158_531_971_244_797;

/// FEE-1: `treasury = floor(base × min(bps, 10000) / 10000)`,
/// `burned = base − treasury`. Every node genesis fixes `bps = 0`, so the
/// whole base fee burns; the tip goes to the fee collector, which becomes the
/// staking reward pool at the epoch boundary.
#[must_use]
pub fn split(base_fee_paid: u64, treasury_bps: u64) -> (u64, u64) {
    let bps = u128::from(treasury_bps.min(BPS));
    let treasury = u128::from(base_fee_paid) * bps / u128::from(BPS);
    let treasury = u64::try_from(treasury).unwrap_or(u64::MAX);
    (base_fee_paid - treasury, treasury)
}

/// FEE-2 .. FEE-4 and FEE-6: the next base fee from the parent's fee and size.
///
/// - FEE-2: a zero target or denominator leaves the fee at `max(parent, floor)`.
/// - FEE-3: over target, it rises by `min(max(1, parent × gap / target / denom),
///   max(1, parent / denom))`; under target, it falls by
///   `parent × gap / target / denom`, saturating at zero; at target it is unchanged.
/// - FEE-6: a rise ends at `max(MAX_BASE_FEE_PER_BYTE, parent)`.
/// - FEE-4: the result is never below `floor`.
#[must_use]
pub fn next_base_fee(
    parent: Balance,
    size: u64,
    target: u64,
    denom: u64,
    floor: Balance,
) -> Balance {
    if target == 0 || denom == 0 {
        return parent.max(floor);
    }
    // parent × gap / target / denom, as one division: nested floors agree.
    let divisor = u128::from(target) * u128::from(denom);
    let delta = parent
        .mul_div_small(size.abs_diff(target), divisor)
        .unwrap_or(Balance::MAX);
    let next = if size > target {
        let share = parent
            .mul_div_small(1, u128::from(denom))
            .unwrap_or(Balance::MAX)
            .max(Balance::from(1));
        let rise = delta.max(Balance::from(1)).min(share);
        let ceiling = Balance::from(MAX_BASE_FEE_PER_BYTE).max(parent);
        parent
            .checked_add(rise)
            .unwrap_or(Balance::MAX)
            .min(ceiling)
    } else {
        parent.checked_sub(delta).unwrap_or_default()
    };
    next.max(floor)
}

/// FEE-5: a transaction is admitted only if `max_fee ≥ base_fee × size_bytes`
/// (computed without overflow).
#[must_use]
pub fn admits(base_fee: u64, size_bytes: u64, max_fee: u64) -> bool {
    u128::from(max_fee) >= u128::from(base_fee) * u128::from(size_bytes)
}

/// FEE-7: a mining share is exempt from the base fee only when it carries no
/// inputs and no outputs. Any other share pays the base fee on its size.
#[must_use]
pub const fn share_fee_exempt(inputs: usize, outputs: usize) -> bool {
    inputs == 0 && outputs == 0
}

/// FEE-7: the base fee a share of `size_bytes` owes at `base_fee`. Zero for an
/// exempt share; otherwise the FEE-5 amount. A share whose fee-collector
/// outputs fall short of this makes its block invalid (`FeeTooLow`).
#[must_use]
pub fn share_fee_due(inputs: usize, outputs: usize, base_fee: u64, size_bytes: u64) -> u128 {
    if share_fee_exempt(inputs, outputs) {
        0
    } else {
        u128::from(base_fee) * u128::from(size_bytes)
    }
}
