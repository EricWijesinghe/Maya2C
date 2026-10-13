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

use crate::error::Result;
use crate::state::db::{Overlay, StateDB};
use crate::state::dex::{POOL_PREFIX, PoolRecord};
use crate::state::invariant_guard::breaker::Invariant;
use crate::state::invariant_guard::limits::{
    BPS_DENOMINATOR, POOL_VALUE_TOLERANCE_BPS, SHIELDED_DRAIN_BPS, SHIELDED_DRAIN_FLOOR,
};
// Shielded and transparent balances (ADR-046).
use primitive_types::U256 as Balance;

/// Limbs in the widest product the pool check forms: `k · s² · bps` is at most
/// 512 + 512 + 64 bits, with reserves and shares 256-bit (ADR-046).
const PRODUCT_LIMBS: usize = 17;

/// A non-negative integer of up to [`PRODUCT_LIMBS`] little-endian limbs.
type Product = [u64; PRODUCT_LIMBS];

/// `a × b`, schoolbook, into a fixed-width product.
///
/// Exact integer arithmetic on the consensus path, written out rather than
/// reached for in a crate: a dependency would be a third party's rounding
/// decisions. No allocation. `a.len() + b.len()` must not exceed
/// [`PRODUCT_LIMBS`]; every caller here passes widths that sum to at most 17.
fn mul_limbs(a: &[u64], b: &[u64]) -> Product {
    let mut out = [0u64; PRODUCT_LIMBS];
    for (i, &x) in a.iter().enumerate() {
        let mut carry: u128 = 0;
        for (j, &y) in b.iter().enumerate() {
            // x*y + out + carry < 2^128: (2^64-1)^2 + 2(2^64-1) = 2^128-1.
            let cur = u128::from(x) * u128::from(y) + u128::from(out[i + j]) + carry;
            out[i + j] = cur as u64;
            carry = cur >> 64;
        }
        if let Some(slot) = out.get_mut(i + b.len()) {
            *slot = carry as u64;
        }
    }
    out
}

/// `value + 1`; the products here are far below 2^1088, so it never carries
/// out of the top limb.
fn plus_one(mut value: Product) -> Product {
    for limb in &mut value {
        let (sum, carried) = limb.overflowing_add(1);
        *limb = sum;
        if !carried {
            break;
        }
    }
    value
}

/// Numeric order of two products: most significant limb first.
fn cmp_products(a: &Product, b: &Product) -> core::cmp::Ordering {
    a.iter().rev().cmp(b.iter().rev())
}

/// The constant-product invariant `k` and the share count squared `s²`, each
/// a 512-bit value.
fn value_per_share(record: &PoolRecord) -> ([u64; 8], [u64; 8]) {
    let k = record.pool.reserve_base.mul_wide(record.pool.reserve_quote);
    let s2 = record.pool.total_shares.mul_wide(record.pool.total_shares);
    (k.0, s2.0)
}

/// Whether `k_after/s²_after` fell below `(1 − tolerance) · k_before/s²_before`.
///
/// The same comparison as before ADR-046, `k_after·s²_before <
/// floor(k_before·s²_after·(B − t) / B)`, written without the division:
/// `X < floor(Y / B)` exactly when `(X + 1)·B ≤ Y`, so the rounding is
/// unchanged.
fn value_fell(after: &PoolRecord, before: &PoolRecord) -> bool {
    let (k_after, s2_after) = value_per_share(after);
    let (k_before, s2_before) = value_per_share(before);
    let x = mul_limbs(&k_after, &s2_before);
    let y = mul_limbs(&k_before, &s2_after);
    let lhs = mul_limbs(&plus_one(x)[..16], &[BPS_DENOMINATOR]);
    let rhs = mul_limbs(&y[..16], &[BPS_DENOMINATOR - POOL_VALUE_TOLERANCE_BPS]);
    cmp_products(&lhs, &rhs) != core::cmp::Ordering::Greater
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
            if after.pool.total_shares.is_zero() || before.pool.total_shares.is_zero() {
                continue;
            }

            // k_after/s2_after >= k_before/s2_before, cross-multiplied, with
            // the tolerance applied to the side it would excuse.
            if value_fell(&after, &before) {
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
        if before < Balance::from(SHIELDED_DRAIN_FLOOR) {
            return Ok(false);
        }
        // A product of two balances needs a 512-bit result, which `full_mul`
        // returns exactly, so the comparison cannot wrap at any balance.
        Ok(withdrawn.full_mul(Balance::from(BPS_DENOMINATOR))
            > before.full_mul(Balance::from(SHIELDED_DRAIN_BPS)))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use maya_dex::Amount;
    use maya_dex::amm::Pool;
    use maya_dex::fees::FeeSchedule;

    fn record(base: Amount, quote: Amount, shares: Amount) -> PoolRecord {
        PoolRecord {
            base_asset: [1; 32],
            quote_asset: [2; 32],
            pool: Pool {
                reserve_base: base,
                reserve_quote: quote,
                total_shares: shares,
                fees: FeeSchedule::standard(),
            },
        }
    }

    #[test]
    fn the_limb_product_agrees_with_u128_and_carries() {
        let p = mul_limbs(&[u64::MAX], &[u64::MAX]);
        let expected = u128::from(u64::MAX) * u128::from(u64::MAX);
        assert_eq!(p[0], expected as u64);
        assert_eq!(p[1], (expected >> 64) as u64);
        assert!(p[2..].iter().all(|&l| l == 0));
        // (2^128 - 1)^2 = 2^256 - 2^129 + 1.
        let q = mul_limbs(&[u64::MAX, u64::MAX], &[u64::MAX, u64::MAX]);
        assert_eq!(q[..4], [1, 0, u64::MAX - 1, u64::MAX]);
    }

    #[test]
    fn an_unchanged_or_richer_pool_has_not_fallen() {
        let a = Amount::from_u64;
        let before = record(a(1_000), a(1_000), a(1_000));
        assert!(!value_fell(&before, &before));
        assert!(!value_fell(&record(a(1_100), a(1_000), a(1_000)), &before));
    }

    #[test]
    fn a_pool_that_lost_value_per_share_has_fallen_at_any_width() {
        let a = Amount::from_u64;
        assert!(value_fell(
            &record(a(500), a(1_000), a(1_000)),
            &record(a(1_000), a(1_000), a(1_000))
        ));
        // ADR-046: reserves near 2^255 need the full 1088-bit product.
        let big = Amount::MAX.shr(1);
        let half = Amount::MAX.shr(2);
        assert!(value_fell(&record(half, big, big), &record(big, big, big)));
        assert!(!value_fell(&record(big, big, big), &record(big, big, big)));
    }
}
