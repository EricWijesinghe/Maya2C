//! The fee estimator a wallet calls (Master Prompt 18 §5), and its accuracy
//! measured against the chain's own base-fee rule on simulated traffic.
//!
//! The estimate for inclusion within `k` blocks is the most the base fee can
//! reach in `k` steps if every block were full — `base · (1 + 1/d)^k` — which
//! is a **guaranteed maximum** under the fee rule, not a guess (Master
//! Prompt 22 §3's "fee quote guaranteed as a maximum"). Accuracy is how much
//! higher that ceiling is than what the fee actually became.

use maya_fee_market::next_base_fee;

/// The highest base fee reachable `k` blocks after `base`, under the rule.
pub fn max_base_fee_after(base: u64, k: u32, target: u64, denominator: u64, floor: u64) -> u64 {
    let mut fee = base;
    for _ in 0..k {
        fee = next_base_fee(fee, 2 * target, target, denominator, floor);
    }
    fee
}

/// Accuracy over a traffic trace of block sizes: the ceiling is never
/// exceeded (`violations` must be 0) and `mean_overestimate_ppm` says how
/// conservative it was.
#[derive(Clone, Copy, Debug)]
pub struct Accuracy {
    /// Samples measured.
    pub samples: u64,
    /// Times the realised fee exceeded the quoted ceiling.
    pub violations: u64,
    /// Mean of (quote − realised) / realised, ppm.
    pub mean_overestimate_ppm: u64,
}

/// Replays `sizes` through the base-fee rule and scores `k`-block quotes.
pub fn measure(
    sizes: &[u64],
    k: u32,
    target: u64,
    denominator: u64,
    floor: u64,
    start: u64,
) -> Accuracy {
    let mut fees = Vec::with_capacity(sizes.len() + 1);
    let mut fee = start;
    fees.push(fee);
    for s in sizes {
        fee = next_base_fee(fee, *s, target, denominator, floor);
        fees.push(fee);
    }
    let k_us = k as usize;
    let (mut violations, mut over, mut samples) = (0u64, 0u128, 0u64);
    for i in 0..fees.len().saturating_sub(k_us) {
        let quote = max_base_fee_after(fees[i], k, target, denominator, floor);
        let realised = fees[i + k_us];
        if realised > quote {
            violations += 1;
        }
        over +=
            u128::from(quote.saturating_sub(realised)) * 1_000_000 / u128::from(realised.max(1));
        samples += 1;
    }
    Accuracy {
        samples,
        violations,
        mean_overestimate_ppm: u64::try_from(over / u128::from(samples.max(1))).unwrap_or(u64::MAX),
    }
}
