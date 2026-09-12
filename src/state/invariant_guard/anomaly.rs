//! The anomaly checks: conditions a block may satisfy the rules and still be
//! wrong about.
//!
//! [Conservation](super::conservation) refuses a block that creates value.
//! These two refuse nothing — the block commits — and instead halt the module
//! that produced the anomaly for [`BREAKER_BLOCKS`] blocks. The split is the
//! whole design: a conservation failure has no innocent reading, and an
//! anomaly does.
//!
//! [`BREAKER_BLOCKS`]: super::limits::BREAKER_BLOCKS

use crate::consensus::uint::U256;
use crate::error::Result;
use crate::state::db::{Overlay, StateDB};
use crate::state::dex::{POOL_PREFIX, PoolRecord};
use crate::state::invariant_guard::breaker::Invariant;
use crate::state::invariant_guard::limits::{
    BPS_DENOMINATOR, POOL_VALUE_TOLERANCE_BPS, SHIELDED_DRAIN_BPS, SHIELDED_DRAIN_FLOOR,
};

/// Low 64 bits.
const LIMB: u128 = u64::MAX as u128;

/// `a * b` as a 256-bit value, high limb first.
///
/// Needed because the pool comparison multiplies a reserve product by a share
/// count squared, and both of those are already 128-bit. Written out rather
/// than reached for in a crate: it is twenty lines of exact integer
/// arithmetic on the consensus path, where a dependency would be a third
/// party's rounding decisions.
fn mul_wide(a: u128, b: u128) -> (u128, u128) {
    let (a_hi, a_lo) = (a >> 64, a & LIMB);
    let (b_hi, b_lo) = (b >> 64, b & LIMB);

    let lo_lo = a_lo * b_lo;
    let hi_lo = a_hi * b_lo;
    let lo_hi = a_lo * b_hi;
    let hi_hi = a_hi * b_hi;

    let mid = (lo_lo >> 64) + (hi_lo & LIMB) + (lo_hi & LIMB);
    let lo = (mid << 64) | (lo_lo & LIMB);
    let hi = hi_hi + (hi_lo >> 64) + (lo_hi >> 64) + (mid >> 64);
    (hi, lo)
}

/// A 256-bit product as a [`U256`], so the tolerance can be applied to it.
fn wide(a: u128, b: u128) -> U256 {
    let (hi, lo) = mul_wide(a, b);
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(&hi.to_be_bytes());
    bytes[16..].copy_from_slice(&lo.to_be_bytes());
    U256::from_be_bytes(&bytes)
}

/// The constant-product invariant: the product of the reserves.
fn constant_product(record: &PoolRecord) -> u128 {
    u128::from(record.pool.reserve_base) * u128::from(record.pool.reserve_quote)
}

/// The value one share redeems for, expressed so it can be compared exactly.
///
/// The quantity is `k / s²`. Comparing two of them as fractions means
/// cross-multiplying, which is what the caller does: there is no division here,
/// so there is no rounding for a caller to disagree about.
fn value_per_share(record: &PoolRecord) -> (u128, u128) {
    let shares = u128::from(record.pool.total_shares);
    (constant_product(record), shares * shares)
}

impl StateDB {
    /// Every anomaly check, in a fixed order.
    ///
    /// Returns the breaker each one tripped, so the caller can log them. The
    /// order is fixed because a block that trips two must trip them in the same
    /// order on every node, or two nodes write two different sets of records
    /// and the state roots differ.
    ///
    /// # Errors
    ///
    /// Returns a read or decode failure from committed state. An anomaly itself
    /// is not an error: it is a record.
    pub(crate) fn check_anomalies(&self, overlay: &mut Overlay, height: u64) -> Result<()> {
        if self.pool_value_fell(overlay)? {
            StateDB::trip_breaker(
                overlay,
                super::Module::Dex,
                Invariant::PoolValuePerShare,
                height,
            );
        }
        if self.shielded_drained_too_fast(overlay)? {
            StateDB::trip_breaker(
                overlay,
                super::Module::Shielded,
                Invariant::ShieldedDrainRate,
                height,
            );
        }
        Ok(())
    }

    /// Whether any pool this block touched is worth less per share than it was.
    ///
    /// Under every operation this chain has, `k / s²` may only rise — see
    /// [`POOL_VALUE_TOLERANCE_BPS`]. So a fall means value left a pool without
    /// a share being burned for it, which is the AMM-manipulation class the
    /// brief names, whatever the mechanism turns out to be.
    ///
    /// A pool being created or fully redeemed is skipped rather than tolerated:
    /// with no shares on one side of the comparison there is no value per share
    /// to compare, and the conservation equation already covers where the
    /// reserves went.
    fn pool_value_fell(&self, overlay: &Overlay) -> Result<bool> {
        for (key, staged) in &overlay.records {
            if !key.starts_with(POOL_PREFIX) {
                continue;
            }
            let (Some(staged), Some(previous)) = (staged.as_deref(), self.raw_get(key)?) else {
                continue;
            };
            let after = PoolRecord::decode(staged)?;
            let before = PoolRecord::decode(&previous)?;
            // A pool being created or fully redeemed has no value per share on
            // one side of the comparison, and conservation already covers where
            // the reserves went. This is only ever the creation case in
            // practice: `MINIMUM_LIQUIDITY` shares are minted to nobody on a
            // first deposit and `remove_liquidity` cannot burn them, so a
            // committed pool's `total_shares` never legitimately returns to
            // zero. Written as `== 0` rather than `< MINIMUM_LIQUIDITY` on
            // purpose — if a future withdrawal path ever did burn into the
            // floor, that pool should be checked, not skipped.
            if after.pool.total_shares == 0 || before.pool.total_shares == 0 {
                continue;
            }

            let (k_after, s2_after) = value_per_share(&after);
            let (k_before, s2_before) = value_per_share(&before);

            // k_after/s2_after >= k_before/s2_before, cross-multiplied, with
            // the tolerance applied to the side it would excuse.
            let floor = wide(k_before, s2_after)
                .mul_div_u64(BPS_DENOMINATOR - POOL_VALUE_TOLERANCE_BPS, BPS_DENOMINATOR)
                .unwrap_or(U256::ZERO);
            if wide(k_after, s2_before) < floor {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether the shielded pool emptied faster than one block is allowed to
    /// empty it.
    ///
    /// A rate, not a rule, and the reasoning is on
    /// [`SHIELDED_DRAIN_BPS`]: conservation balances perfectly across the pool
    /// boundary even when the value leaving it was minted by a broken proof, so
    /// the rate is the only thing the transparent chain can see.
    ///
    /// The rate applies only above [`SHIELDED_DRAIN_FLOOR`]. A percentage of a
    /// thin pool is a small absolute number, and a breaker anybody can trip for
    /// a small absolute number is a way to halt every other user's shielded
    /// transactions rather than a way to catch a break.
    fn shielded_drained_too_fast(&self, overlay: &Overlay) -> Result<bool> {
        let Some(pool) = &overlay.shielded else {
            return Ok(false);
        };
        let before = self.stored_pool()?.balance();
        let after = pool.balance();
        let Some(withdrawn) = before.checked_sub(after) else {
            return Ok(false);
        };
        if before < SHIELDED_DRAIN_FLOOR {
            return Ok(false);
        }
        // `before` is at least `withdrawn`, so neither product can overflow a
        // u128 and neither needs saturating.
        Ok(u128::from(withdrawn) * u128::from(BPS_DENOMINATOR)
            > u128::from(before) * u128::from(SHIELDED_DRAIN_BPS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wide_product_agrees_with_u128_where_u128_can_hold_it() {
        for (a, b) in [(0u128, 0u128), (1, 1), (u64::MAX as u128, u64::MAX as u128)] {
            assert_eq!(mul_wide(a, b), (0, a * b), "{a} * {b}");
        }
    }

    #[test]
    fn the_wide_product_carries_past_128_bits() {
        let (hi, lo) = mul_wide(u128::MAX, u128::MAX);
        // (2^128 - 1)^2 = 2^256 - 2^129 + 1
        assert_eq!(hi, u128::MAX - 1);
        assert_eq!(lo, 1);
    }

    #[test]
    fn a_wider_product_orders_above_a_narrower_one() {
        assert!(wide(u128::MAX, 2) > wide(u128::MAX, 1));
        assert!(wide(0, u128::MAX) == U256::ZERO);
    }
}
