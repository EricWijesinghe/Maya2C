//! How concentrated stake is (Master Prompt 18 §5), in integers.
//!
//! Computed every epoch by the node and served over RPC. No floats: these
//! numbers are not consensus, but they are published, and a published number
//! that differs between two honest nodes is a bug report nobody can close.

use alloc::vec::Vec;

use crate::params::BPS;
use crate::types::Stake;

/// Concentration of one stake distribution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Concentration {
    /// Fewest validators whose stake together exceeds one third of the total:
    /// the smallest coalition that can halt a BFT chain. 0 for no stake.
    pub nakamoto_halt: usize,
    /// Fewest validators whose stake exceeds two thirds: the smallest
    /// coalition that can finalize alone.
    pub nakamoto_finalize: usize,
    /// Gini coefficient in basis points (0 = equal, 10 000 = one holds all).
    pub gini_bps: u64,
    /// Largest single validator's share, in basis points.
    pub top_share_bps: u64,
}

/// Concentration of `stakes` (any order).
#[must_use]
pub fn concentration(stakes: &[Stake]) -> Concentration {
    let mut amounts: Vec<u128> = stakes.iter().map(|s| u128::from(s.amount)).collect();
    amounts.sort_unstable_by(|a, b| b.cmp(a));
    let total: u128 = amounts.iter().sum();
    if total == 0 {
        return Concentration {
            nakamoto_halt: 0,
            nakamoto_finalize: 0,
            gini_bps: 0,
            top_share_bps: 0,
        };
    }
    Concentration {
        nakamoto_halt: coalition(&amounts, total, 1, 3),
        nakamoto_finalize: coalition(&amounts, total, 2, 3),
        gini_bps: gini_bps(&amounts, total),
        top_share_bps: to_u64(amounts[0] * u128::from(BPS) / total),
    }
}

/// Fewest of `desc` (largest first) whose sum exceeds `num/den` of `total`.
fn coalition(desc: &[u128], total: u128, num: u128, den: u128) -> usize {
    let mut sum = 0u128;
    for (i, a) in desc.iter().enumerate() {
        sum += a;
        if sum * den > total * num {
            return i + 1;
        }
    }
    desc.len()
}

/// Gini over `desc` (largest first): `Σ (2i − n − 1)·x_i / (n·Σx)` with `x`
/// ascending and `i` from 1, scaled to basis points.
fn gini_bps(desc: &[u128], total: u128) -> u64 {
    let n = desc.len() as u128;
    if n < 2 {
        return 0;
    }
    // Stakes are `u64` and there are at most `u16::MAX` validators, so every
    // term fits `i128` with room to spare; `try_from` only makes that checked.
    let signed = |v: u128| i128::try_from(v).unwrap_or(i128::MAX);
    let mut weighted: i128 = 0;
    for (k, x) in desc.iter().rev().enumerate() {
        let i = signed(k as u128) + 1;
        let coeff = 2 * i - signed(n) - 1;
        weighted += coeff * signed(*x);
    }
    let num = u128::try_from(weighted.max(0)).unwrap_or(0) * u128::from(BPS);
    to_u64(num / (n * total))
}

fn to_u64(v: u128) -> u64 {
    u64::try_from(v).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stakes(amounts: &[u64]) -> Vec<Stake> {
        amounts
            .iter()
            .enumerate()
            .map(|(i, a)| Stake {
                id: [u8::try_from(i).unwrap_or(0); 32],
                amount: *a,
            })
            .collect()
    }

    #[test]
    fn equal_stake_has_zero_gini_and_the_bft_coalition_sizes() {
        let c = concentration(&stakes(&[100; 10]));
        assert_eq!(c.gini_bps, 0);
        // 4 of 10 exceed a third; 7 of 10 exceed two thirds.
        assert_eq!((c.nakamoto_halt, c.nakamoto_finalize), (4, 7));
        assert_eq!(c.top_share_bps, 1_000);
    }

    #[test]
    fn one_whale_dominates_every_measure() {
        let c = concentration(&stakes(&[1_000_000, 1, 1, 1]));
        assert_eq!((c.nakamoto_halt, c.nakamoto_finalize), (1, 1));
        assert!(c.gini_bps > 7_000, "gini {}", c.gini_bps);
        assert!(c.top_share_bps > 9_990);
    }

    #[test]
    fn nothing_staked_is_all_zero() {
        let c = concentration(&[]);
        assert_eq!((c.nakamoto_halt, c.gini_bps), (0, 0));
    }
}
