//! Mining reward schedule — `spec/09-mining.md` MINE-1 .. MINE-10 (ADR-044, ADR-068).
//!
//! Written from the spec text, not from `custom_l1_node::state::mining`, which
//! is what these functions are checked against.

use crate::Address;
use crate::balance::Balance;

/// Basis points in 100 %.
pub const BPS: u64 = 10_000;

/// MINE-5's factor: the retarget moves at most this much per epoch, either way.
const RETARGET_FACTOR: u64 = 4;

/// Domain of MINE-9's share header.
const SHARE_DOMAIN: &str = "maya2c mining share v1";

/// MINE-3's ceiling, 2^192.
fn ceiling() -> Balance {
    // 2^192 as three steps of 2^64.
    (0..3).fold(Balance::from(1), |b, _| {
        b.checked_mul_count(1 << 64).unwrap_or(Balance::MAX)
    })
}

/// MINE-1: `max(⌊initial / 2^⌊epoch / halving⌋⌋, tail)`.
#[must_use]
pub fn epoch_reward(initial: Balance, tail: Balance, halving_epochs: u64, epoch: u64) -> Balance {
    initial.shr_bits(epoch / halving_epochs.max(1)).max(tail)
}

/// MINE-2: the miners' part `⌊reward × min(bps, 10000) / 10000⌋` and the
/// committee's part, the rest.
#[must_use]
pub fn split(reward: Balance, miner_bps: u64) -> (Balance, Balance) {
    let miners = reward
        .mul_div_small(miner_bps.min(BPS), u128::from(BPS))
        .unwrap_or(reward);
    (miners, reward.checked_sub(miners).unwrap_or_default())
}

/// MINE-3: the sum of [`epoch_reward`] over every epoch `0 ..= ⌊(2^64 − 1) /
/// epoch_blocks⌋`, or `None` when it reaches 2^192.
#[must_use]
pub fn lifetime_emission(
    initial: Balance,
    tail: Balance,
    halving_epochs: u64,
    epoch_blocks: u64,
) -> Option<Balance> {
    let epochs = u128::from(u64::MAX / epoch_blocks.max(1)) + 1;
    let era = u128::from(halving_epochs.max(1));
    let mut total = Balance::from(0);
    let mut start = 0u128;
    let mut halvings = 0u64;
    while start < epochs {
        let halved = initial.shr_bits(halvings);
        let left = epochs - start;
        if halved <= tail {
            total = total.checked_add(tail.checked_mul_count(left)?)?;
            break;
        }
        total = total.checked_add(halved.checked_mul_count(era.min(left))?)?;
        start += era;
        halvings += 1;
    }
    (total < ceiling()).then_some(total)
}

/// MINE-4: one share's payout, `min(⌊budget / target_shares⌋, budget − paid)`,
/// where `budget` is MINE-2's miners' part of `reward` and `paid` is what the
/// epoch has paid out so far. Zero once the budget is spent.
#[must_use]
pub fn share_reward(reward: Balance, miner_bps: u64, target_shares: u64, paid: Balance) -> Balance {
    if target_shares == 0 {
        return Balance::from(0);
    }
    let (budget, _) = split(reward, miner_bps);
    let per_share = budget
        .mul_div_small(1, u128::from(target_shares))
        .unwrap_or(Balance::from(0));
    per_share.min(budget.checked_sub(paid).unwrap_or_default())
}

/// MINE-5: the next share target. `target × target_shares / observed`, where
/// `observed` is the epoch's demand clamped to a factor of 4 either way
/// (`max(target_shares / 4, 1)` to `4 × target_shares`). Demand is the share
/// count, scaled up by `epoch_blocks / spent_after` when the budget ran out
/// early. The result is capped at `limit`, and never zero.
#[must_use]
pub fn retarget(
    target: Balance,
    limit: Balance,
    target_shares: u64,
    shares: u64,
    spent_after: u64,
    epoch_blocks: u64,
) -> Balance {
    let demand = if spent_after == 0 {
        shares
    } else {
        let scaled = u128::from(shares) * u128::from(epoch_blocks) / u128::from(spent_after);
        u64::try_from(scaled).unwrap_or(u64::MAX)
    };
    let low = (target_shares / RETARGET_FACTOR).max(1);
    let observed = demand.clamp(low, target_shares.saturating_mul(RETARGET_FACTOR));
    let next = target
        .mul_div_small(target_shares, u128::from(observed))
        .unwrap_or(limit);
    if next > limit {
        limit
    } else if next == Balance::from(0) {
        Balance::from(1)
    } else {
        next
    }
}

/// The parameters a genesis or a decoded mining record carries, as MINE-6 to
/// MINE-8 check them. Field names follow `spec/09-mining.md`.
#[derive(Clone, Copy, Debug)]
pub struct Schedule {
    pub initial: Balance,
    pub tail: Balance,
    pub halving_epochs: u64,
    pub target_shares: u64,
    pub epoch_blocks: u64,
    pub max_shares_per_block: u64,
    pub miner_bps: u64,
    pub spent_after: u64,
    pub target: Balance,
    pub target_limit: Balance,
}

/// MINE-6 to MINE-8: `Ok` when the schedule is accepted, otherwise `Err` with
/// the rule that refuses it. The checks run in the order of the rules.
pub fn validate(s: &Schedule) -> Result<(), &'static str> {
    let zero = Balance::from(0);
    if s.halving_epochs == 0
        || s.target_shares == 0
        || s.epoch_blocks == 0
        || s.max_shares_per_block == 0
        || s.miner_bps > BPS
        || s.tail > s.initial
        || s.spent_after > s.epoch_blocks
    {
        return Err("MINE-6");
    }
    if s.miner_bps > 0 {
        let first = share_reward(s.initial, s.miner_bps, s.target_shares, zero);
        let tail = share_reward(s.tail, s.miner_bps, s.target_shares, zero);
        if first == zero || (s.tail != zero && tail == zero) {
            return Err("MINE-7");
        }
    }
    if lifetime_emission(s.initial, s.tail, s.halving_epochs, s.epoch_blocks).is_none() {
        return Err("MINE-3");
    }
    if s.target == zero || s.target > s.target_limit {
        return Err("MINE-8");
    }
    Ok(())
}

/// MINE-9: the header a share's proof of work commits to, a BLAKE3 derive-key
/// hash of `chain_tag ‖ epoch (u64 LE) ‖ anchor ‖ miner`. Binds the chain, the
/// epoch, the epoch's anchor and the payee.
#[must_use]
pub fn share_header(
    chain_tag: &[u8; 32],
    epoch: u64,
    anchor: &[u8; 32],
    miner: &Address,
) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(SHARE_DOMAIN);
    hasher.update(chain_tag);
    hasher.update(&epoch.to_le_bytes());
    hasher.update(anchor);
    hasher.update(miner);
    *hasher.finalize().as_bytes()
}

/// MINE-10: a share's digest is a share when it is at or below the epoch's
/// target, both read as 256-bit big-endian integers.
#[must_use]
pub fn meets_target(digest: &[u8; 32], target: &[u8; 32]) -> bool {
    digest <= target
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_sum_matches_an_epoch_by_epoch_count() {
        let (initial, tail, h, blocks) = (
            Balance::from(1_000_000),
            Balance::from(100_000),
            10,
            u64::MAX / 50,
        );
        let last = u64::MAX / blocks;
        let brute = (0..=last).fold(Balance::from(0), |s, e| {
            s.checked_add(epoch_reward(initial, tail, h, e)).unwrap()
        });
        assert_eq!(lifetime_emission(initial, tail, h, blocks), Some(brute));
    }

    #[test]
    fn shifting_and_counting_are_exact_at_the_edges() {
        assert_eq!(Balance::MAX.shr_bits(256), Balance::from(0));
        assert_eq!(Balance::MAX.shr_bits(255), Balance::from(1));
        assert_eq!(
            Balance::from(1)
                .checked_mul_count(1 << 64)
                .unwrap()
                .shr_bits(64),
            Balance::from(1)
        );
        assert_eq!(Balance::MAX.checked_mul_count(2), None);
    }
}
