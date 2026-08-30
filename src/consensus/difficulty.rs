//! Difficulty retargeting and cumulative chain work.
//!
//! ## Retargeting
//!
//! Every [`RETARGET_INTERVAL`] blocks the target is rescaled by how far the
//! window's actual elapsed time diverged from the intended
//! `RETARGET_INTERVAL × TARGET_BLOCK_TIME`:
//!
//! ```text
//! new_target = old_target × actual_timespan ÷ expected_timespan
//! ```
//!
//! A larger target is easier, so a window that ran slow (large actual timespan)
//! raises the target and speeds the chain back up.
//!
//! The ratio is clamped to `[1/4, 4]` per retarget. That bound is what stops a
//! single window — whether from an honest hashrate cliff or a manipulated
//! timestamp — from moving difficulty arbitrarily far in one step.
//!
//! ## Work
//!
//! Comparing branches by height is wrong: a long chain of easy blocks must lose
//! to a shorter chain of hard ones. The comparable quantity is *expected hashes
//! spent*, which for a target `t` is `2²⁵⁶ ÷ (t + 1)`.

use crate::consensus::uint::U256;

/// Blocks between difficulty recalculations.
pub const RETARGET_INTERVAL: u64 = 100;

/// Intended seconds per block.
pub const TARGET_BLOCK_TIME: u64 = 15;

/// Intended seconds per retarget window.
pub const EXPECTED_TIMESPAN: u64 = RETARGET_INTERVAL * TARGET_BLOCK_TIME;

/// Maximum factor by which difficulty may move in a single retarget.
pub const MAX_ADJUSTMENT_FACTOR: u64 = 4;

/// Default easiest permitted target: 2²²⁴ − 1, i.e. 32 leading zero bits.
///
/// This is a *chain parameter*, not a universal constant — the same role
/// Bitcoin's `powLimit` plays, where mainnet and regtest use different values.
/// A test or private network needs a far easier floor, so it is carried in
/// [`crate::consensus::ChainConfig`] rather than hard-coded into the retarget
/// rule. Clamping every target to a mainnet-strength floor would reject
/// perfectly valid low-difficulty chains.
#[must_use]
pub fn default_pow_limit() -> [u8; 32] {
    let mut limit = [0xFFu8; 32];
    limit[0] = 0x00;
    limit[1] = 0x00;
    limit[2] = 0x00;
    limit[3] = 0x00;
    limit
}

/// The easiest representable target, for networks with no meaningful floor.
#[must_use]
pub fn unlimited_pow_limit() -> [u8; 32] {
    [0xFFu8; 32]
}

/// Clamps a window's elapsed time to the permitted adjustment band.
///
/// Applied to the timespan rather than to the resulting target because it keeps
/// the bound exact: dividing targets would round.
#[must_use]
pub fn clamp_timespan(actual_timespan: u64) -> u64 {
    let min = EXPECTED_TIMESPAN / MAX_ADJUSTMENT_FACTOR;
    let max = EXPECTED_TIMESPAN.saturating_mul(MAX_ADJUSTMENT_FACTOR);
    actual_timespan.clamp(min, max)
}

/// Computes the next target from the previous one and the window's elapsed time.
///
/// `actual_timespan` is the number of seconds the previous
/// [`RETARGET_INTERVAL`] blocks took. The result is clamped to `pow_limit` so
/// difficulty can never drift below the network's floor.
#[must_use]
pub fn retarget(
    previous_target: &[u8; 32],
    actual_timespan: u64,
    pow_limit: &[u8; 32],
) -> [u8; 32] {
    let clamped = clamp_timespan(actual_timespan);
    let previous = U256::from_be_bytes(previous_target);
    let limit = U256::from_be_bytes(pow_limit);

    // Overflow here means the target grew past 256 bits, which is easier than
    // the floor by definition — fall back to the limit.
    let next = match previous.mul_div_u64(clamped, EXPECTED_TIMESPAN) {
        Some(value) => value,
        None => limit,
    };

    if next > limit {
        limit.to_be_bytes()
    } else if next.is_zero() {
        // A zero target is unsatisfiable by any digest except all-zero; treat
        // it as the hardest representable target instead.
        U256::ONE.to_be_bytes()
    } else {
        next.to_be_bytes()
    }
}

/// Whether a retarget occurs at `height`.
///
/// Height 0 is genesis and never retargets.
#[must_use]
pub fn is_retarget_height(height: u64) -> bool {
    height > 0 && height.is_multiple_of(RETARGET_INTERVAL)
}

/// Expected number of hashes needed to satisfy `target`.
///
/// Computed as `2²⁵⁶ ÷ (target + 1)`. The numerator does not fit in 256 bits,
/// so this uses the standard identity
/// `2²⁵⁶ ÷ (t+1) == (¬t ÷ (t+1)) + 1`, which stays entirely within 256 bits.
#[must_use]
pub fn work_from_target(target: &[u8; 32]) -> U256 {
    let value = U256::from_be_bytes(target);
    let (divisor, overflow) = value.overflowing_add(U256::ONE);

    // target == 2²⁵⁶−1: every digest satisfies it, so the work is 1.
    if overflow {
        return U256::ONE;
    }

    match (!value).div_rem(divisor) {
        Some((quotient, _)) => quotient.saturating_add(U256::ONE),
        // Unreachable: divisor is non-zero because the add did not overflow.
        None => U256::ONE,
    }
}

/// Sums the work of a sequence of targets, saturating at [`U256::MAX`].
#[must_use]
pub fn cumulative_work<'a, I>(targets: I) -> U256
where
    I: IntoIterator<Item = &'a [u8; 32]>,
{
    targets.into_iter().fold(U256::ZERO, |total, target| {
        total.saturating_add(work_from_target(target))
    })
}
