//! Fee rules — `spec/04-fees.md` FEE-1 .. FEE-4.

use crate::balance::Balance;

/// Basis points in 100 %.
pub const BPS: u64 = 10_000;

/// FEE-1: `treasury = floor(base × min(bps, 10000) / 10000)`,
/// `burned = base − treasury`; the tip goes whole to the producer.
#[must_use]
pub fn split(base_fee_paid: u64, treasury_bps: u64) -> (u64, u64) {
    let bps = u128::from(treasury_bps.min(BPS));
    let treasury = u128::from(base_fee_paid) * bps / u128::from(BPS);
    let treasury = u64::try_from(treasury).unwrap_or(u64::MAX);
    (base_fee_paid - treasury, treasury)
}

/// FEE-2 .. FEE-4: the next base fee from the parent's fee and size.
///
/// - FEE-2: a zero target or denominator leaves the fee at `max(parent, floor)`.
/// - FEE-3: over target, it rises by `max(1, parent × gap / target / denom)`,
///   saturating at 2^256 − 1; under target, it falls by
///   `parent × gap / target / denom`, saturating at zero; at target it is unchanged.
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
        parent
            .checked_add(delta.max(Balance::from(1)))
            .unwrap_or(Balance::MAX)
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
