//! Mining reward schedule — `spec/09-mining.md` MINE-1 .. MINE-3 (ADR-044, ADR-068).
//!
//! Written from the spec text, not from `custom_l1_node::state::mining`, which
//! is what these functions are checked against.

use crate::balance::Balance;

/// Basis points in 100 %.
pub const BPS: u64 = 10_000;

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
    initial.shr(epoch / halving_epochs.max(1)).max(tail)
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
        let halved = initial.shr(halvings);
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_sum_matches_an_epoch_by_epoch_count() {
        let (initial, tail, h, blocks) = (Balance::from(1_000_000), Balance::from(100_000), 10, u64::MAX / 50);
        let last = u64::MAX / blocks;
        let brute = (0..=last).fold(Balance::from(0), |s, e| s.checked_add(epoch_reward(initial, tail, h, e)).unwrap());
        assert_eq!(lifetime_emission(initial, tail, h, blocks), Some(brute));
    }

    #[test]
    fn shifting_and_counting_are_exact_at_the_edges() {
        assert_eq!(Balance::MAX.shr(256), Balance::from(0));
        assert_eq!(Balance::MAX.shr(255), Balance::from(1));
        assert_eq!(Balance::from(1).checked_mul_count(1 << 64).unwrap().shr(64), Balance::from(1));
        assert_eq!(Balance::MAX.checked_mul_count(2), None);
    }
}
