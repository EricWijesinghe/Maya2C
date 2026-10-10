//! Pay-Per-Last-N-Shares accounting.
//!
//! ## The window is measured in weight, not in shares
//!
//! A share count is the wrong unit the moment vardiff exists. Two miners on
//! different targets submit shares worth different amounts of work, and a
//! window of "the last hundred thousand shares" would pay them the same for
//! them. `N` here is a quantity of *work*, expressed as a multiple of network
//! difficulty, and a share contributes its own weight to filling it.
//!
//! That is also what makes the scheme resistant to the obvious manipulation.
//! Splitting one rig across ten workers produces ten times the shares at a
//! tenth the difficulty each, for the same total weight and the same payout —
//! `tests::splitting_a_farm_across_workers_changes_nothing` states it as an
//! assertion rather than a claim.
//!
//! ## Why the denominator is what accumulated, not `N`
//!
//! A young pool has not yet earned `N` worth of shares. Dividing by `N` anyway
//! would distribute less than the reward and silently strand the difference in
//! the treasury. The denominator is therefore always the weight the window
//! actually contains — which for a full window is `N` plus at most one boundary
//! share, and for a short one is everything there is.
//!
//! ## Rounding, and the invariant that has to hold
//!
//! Integer division loses units. Handing the remainder to nobody would leak
//! value out of every payout; handing it to the largest miner would make the
//! split depend on rounding rather than on work. This uses the largest-remainder
//! method — the fractional parts are ranked and the spare units go to the
//! closest-missed claims, ties broken by address so two runs of the same input
//! produce the same output.
//!
//! The invariant [`split`] enforces is exact: the credits sum to the reward, to
//! the unit. Not approximately, and not with a tolerance.

use std::collections::BTreeMap;

use custom_l1_node::consensus::U256;
use custom_l1_node::consensus::difficulty::work_from_target;
use custom_l1_node::state::Address;

use crate::error::{PoolError, Result};
use crate::ledger::WindowSlice;
use crate::model::PayoutEntry;

/// Denominator used to express the PPLNS factor as a rational.
///
/// The factor is configured as an `f64` because that is how operators think
/// about it, and every use of it here is integer arithmetic. Three decimal
/// places is far finer than the choice is ever made to.
const FACTOR_SCALE: u64 = 1_000;

/// The window size `N`, in units of work.
///
/// `factor` multiplies the work one block at `network_target` represents, so a
/// factor of 2 is a window holding twice a block's expected work.
///
/// # Errors
///
/// Returns [`PoolError::Config`] for a factor that is not a positive, finite
/// number, since a window of zero or NaN work would divide every payout by
/// nothing.
pub fn window_size(network_target: &[u8; 32], factor: f64) -> Result<U256> {
    if !factor.is_finite() || factor <= 0.0 {
        return Err(PoolError::Config(format!(
            "pplns_factor {factor} must be positive and finite"
        )));
    }

    let scaled = (factor * FACTOR_SCALE as f64).round();
    // Saturating rather than casting straight through: an `as` cast from an
    // out-of-range f64 saturates in Rust, but writing it down is what makes the
    // bound visible to the next reader.
    let numerator = scaled.min(u64::MAX as f64) as u64;

    work_from_target(network_target)
        .mul_div_u64(numerator.max(1), FACTOR_SCALE)
        .ok_or_else(|| {
            PoolError::Config(format!(
                "pplns_factor {factor} overflows the window against this target"
            ))
        })
}

/// Splits `reward` across the miners in `window`, proportional to their weight.
///
/// Returns entries with a non-zero amount, ordered by address so the output is
/// a function of the input alone.
///
/// # Errors
///
/// Returns [`PoolError::Treasury`] if the window is empty — there is nobody to
/// pay and the caller must not send the reward anywhere — or if the arithmetic
/// cannot be performed exactly. Both stop the payout rather than approximating
/// it.
pub fn split(window: &WindowSlice, reward: u64) -> Result<Vec<PayoutEntry>> {
    if window.shares.is_empty() || window.total.is_zero() {
        return Err(PoolError::Treasury(
            "the PPLNS window is empty, so there is nobody to credit".to_string(),
        ));
    }

    // Aggregate to the miner, not the worker. A miner's rigs are an operational
    // detail; the address is what gets paid.
    let mut weights: BTreeMap<Address, U256> = BTreeMap::new();
    for share in &window.shares {
        let entry = weights.entry(share.miner).or_insert(U256::ZERO);
        *entry = entry.saturating_add(U256::from_u64(share.weight));
    }

    let denominator = window.total;
    let mut floors: Vec<(Address, u64, U256)> = Vec::with_capacity(weights.len());
    let mut distributed: u64 = 0;

    for (miner, weight) in &weights {
        // `weight * reward` cannot overflow 256 bits by any reachable route: a
        // window holds at most MAX_WINDOW_SHARES shares of at most 2^63 each,
        // which is under 2^86, and a reward is under 2^64. The check is here
        // because an accounting routine that guesses when its arithmetic fails
        // is worse than one that stops.
        let scaled = weight.mul_div_u64(reward, 1).ok_or_else(|| {
            PoolError::Treasury(
                "share weight times reward overflowed 256 bits; refusing to \
                 approximate a payout"
                    .to_string(),
            )
        })?;

        let (quotient, remainder) = scaled
            .div_rem(denominator)
            .ok_or_else(|| PoolError::Treasury("PPLNS window weight is zero".to_string()))?;

        let amount = to_u64(quotient).ok_or_else(|| {
            PoolError::Treasury(
                "a single miner's credit exceeded the reward, which means the \
                 window denominator is wrong"
                    .to_string(),
            )
        })?;

        distributed = distributed
            .checked_add(amount)
            .ok_or_else(|| PoolError::Treasury("credits summed past the reward".to_string()))?;
        floors.push((*miner, amount, remainder));
    }

    // Largest remainder: the spare units go to the claims that missed by most.
    // Ties break on the address so the result does not depend on map ordering.
    let mut spare = reward.saturating_sub(distributed);
    if spare > 0 {
        floors.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        for slot in &mut floors {
            if spare == 0 {
                break;
            }
            slot.1 += 1;
            spare -= 1;
        }
    }

    let mut entries: Vec<PayoutEntry> = floors
        .into_iter()
        .filter(|(_, amount, _)| *amount > 0)
        .map(|(miner, amount, _)| PayoutEntry { miner, amount })
        .collect();
    entries.sort_by_key(|a| a.miner);

    // The invariant, checked rather than asserted in a comment. A payout that
    // does not sum to the reward is either minting or stranding value, and both
    // are worse than a stopped payout task.
    let total: u64 = entries.iter().map(|entry| entry.amount).sum();
    if total != reward {
        return Err(PoolError::Treasury(format!(
            "PPLNS split produced {total} against a reward of {reward}"
        )));
    }

    Ok(entries)
}

/// Narrows a `U256` to a `u64`, or `None` if it does not fit.
fn to_u64(value: U256) -> Option<u64> {
    let bytes = value.to_be_bytes();
    let (high, low) = bytes.split_at(24);
    if high.iter().any(|byte| *byte != 0) {
        return None;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(low);
    Some(u64::from_be_bytes(buf))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    use custom_l1_node::crypto::pow::target_from_leading_zero_bits;

    use crate::ledger::accumulate;
    use crate::model::ShareRecord;

    const ALICE: Address = [0xAA; 32];
    const BOB: Address = [0xBB; 32];
    const CAROL: Address = [0xCC; 32];

    /// Builds a window from `(miner, worker, weight)` triples, newest first.
    fn window(shares: &[(Address, &str, u64)]) -> WindowSlice {
        let records: Vec<ShareRecord> = shares
            .iter()
            .enumerate()
            .map(|(index, (miner, worker, weight))| ShareRecord {
                sequence: index as u64,
                miner: *miner,
                worker: (*worker).to_string(),
                weight: *weight,
                accepted_at_millis: index as u64,
            })
            .collect();
        accumulate(records, U256::MAX)
    }

    /// One miner's credit from a split.
    fn amount_for(entries: &[PayoutEntry], miner: Address) -> u64 {
        entries
            .iter()
            .find(|entry| entry.miner == miner)
            .map_or(0, |entry| entry.amount)
    }

    #[test]
    fn an_even_split_is_even() {
        let slice = window(&[(ALICE, "rig", 100), (BOB, "rig", 100)]);
        let entries = split(&slice, 1_000).unwrap();

        assert_eq!(amount_for(&entries, ALICE), 500);
        assert_eq!(amount_for(&entries, BOB), 500);
    }

    #[test]
    fn credits_are_proportional_to_weight_not_to_share_count() {
        // One heavy share against nine light ones. A count-based window would
        // pay the nine.
        let mut shares = vec![(ALICE, "rig", 900u64)];
        shares.extend((0..9).map(|_| (BOB, "rig", 100u64)));

        let entries = split(&window(&shares), 1_800).unwrap();
        assert_eq!(amount_for(&entries, ALICE), 900);
        assert_eq!(amount_for(&entries, BOB), 900);
    }

    #[test]
    fn splitting_a_farm_across_workers_changes_nothing() {
        // The manipulation the weight-based window exists to defeat: ten
        // workers at a tenth the difficulty is the same work and the same pay.
        let whole = window(&[(ALICE, "rig", 1_000), (BOB, "rig", 1_000)]);

        let mut split_up = vec![(BOB, "rig", 1_000u64)];
        split_up.extend((0..10).map(|_| (ALICE, "rig", 100u64)));

        let before = split(&whole, 4_242).unwrap();
        let after = split(&window(&split_up), 4_242).unwrap();

        assert_eq!(amount_for(&before, ALICE), amount_for(&after, ALICE));
        assert_eq!(amount_for(&before, BOB), amount_for(&after, BOB));
    }

    #[test]
    fn the_credits_always_sum_to_the_reward() {
        // Three-way splits of awkward rewards are where integer division loses
        // units, and where a payout starts minting or stranding value.
        for reward in [1u64, 2, 3, 7, 999, 1_000, 1_000_003, u32::MAX as u64] {
            let slice = window(&[(ALICE, "rig", 1), (BOB, "rig", 1), (CAROL, "rig", 1)]);
            let entries = split(&slice, reward).unwrap();
            let total: u64 = entries.iter().map(|entry| entry.amount).sum();
            assert_eq!(total, reward, "split of {reward} did not conserve value");
        }
    }

    #[test]
    fn an_uneven_split_gives_the_spare_units_to_the_closest_misses() {
        // Reward 10 across weights 1, 1, 1: floors are 3, 3, 3 with a spare
        // unit that must land somewhere deterministic.
        let slice = window(&[(ALICE, "rig", 1), (BOB, "rig", 1), (CAROL, "rig", 1)]);
        let first = split(&slice, 10).unwrap();
        let second = split(&slice, 10).unwrap();

        assert_eq!(first, second, "the split must be a function of its input");
        assert_eq!(first.iter().map(|e| e.amount).sum::<u64>(), 10);
    }

    #[test]
    fn a_short_window_still_distributes_the_whole_reward() {
        // Dividing by N rather than by what accumulated would strand the
        // difference in the treasury with nothing to show it.
        let mut slice = window(&[(ALICE, "rig", 10)]);
        slice.full = false;

        let entries = split(&slice, 1_000).unwrap();
        assert_eq!(amount_for(&entries, ALICE), 1_000);
    }

    #[test]
    fn an_empty_window_is_an_error_rather_than_a_silent_zero() {
        // Nobody to pay means the reward must not be sent anywhere, and the
        // caller has to hear about it.
        let slice = WindowSlice::default();
        assert!(matches!(split(&slice, 1_000), Err(PoolError::Treasury(_))));
    }

    #[test]
    fn a_miner_with_a_rounded_away_credit_is_left_out_rather_than_paid_zero() {
        // A zero-amount output would still cost bytes in the transaction.
        let mut shares = vec![(BOB, "rig", 1u64)];
        shares.extend((0..1_000).map(|_| (ALICE, "rig", 1_000_000u64)));

        let entries = split(&window(&shares), 100).unwrap();
        assert!(entries.iter().all(|entry| entry.amount > 0));
    }

    #[test]
    fn a_window_of_two_difficulty_is_twice_a_blocks_work() {
        let target = target_from_leading_zero_bits(24);
        let single = window_size(&target, 1.0).unwrap();
        let double = window_size(&target, 2.0).unwrap();

        assert_eq!(double, single.saturating_add(single));
        assert_eq!(single, work_from_target(&target));
    }

    #[test]
    fn a_fractional_factor_is_honoured() {
        let target = target_from_leading_zero_bits(20);
        let half = window_size(&target, 0.5).unwrap();
        let whole = window_size(&target, 1.0).unwrap();

        assert_eq!(half.saturating_add(half), whole);
    }

    #[test]
    fn a_non_positive_or_non_finite_factor_is_refused() {
        let target = target_from_leading_zero_bits(20);
        for factor in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                window_size(&target, factor).is_err(),
                "factor {factor} was accepted"
            );
        }
    }
}
