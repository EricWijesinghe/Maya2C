//! Stake concentration (Master Prompt 18 §3): Nakamoto coefficient and Gini,
//! integer-only so the same functions can be exposed over RPC and computed
//! identically by every node at an epoch boundary.

/// The fewest validators whose combined stake exceeds `threshold_ppm` of the
/// total. With `333_334` it is the number that could halt a BFT chain.
pub fn nakamoto_coefficient(stakes: &[u64], threshold_ppm: u64) -> usize {
    let total: u128 = stakes.iter().map(|s| u128::from(*s)).sum();
    if total == 0 {
        return 0;
    }
    let mut sorted = stakes.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    let mut acc = 0u128;
    for (i, s) in sorted.iter().enumerate() {
        acc += u128::from(*s);
        if acc * 1_000_000 > total * u128::from(threshold_ppm) {
            return i + 1;
        }
    }
    sorted.len()
}

/// Gini coefficient of stakes, in ppm (0 = equal, 1,000,000 = one holds all).
pub fn gini_ppm(stakes: &[u64]) -> u64 {
    let n = stakes.len() as u128;
    let total: u128 = stakes.iter().map(|s| u128::from(*s)).sum();
    if n == 0 || total == 0 {
        return 0;
    }
    let mut sorted = stakes.to_vec();
    sorted.sort_unstable();
    // G = (2 Σ i·x_i) / (n Σ x) − (n + 1)/n, with i from 1.
    let weighted: u128 = sorted
        .iter()
        .enumerate()
        .map(|(i, x)| (i as u128 + 1) * u128::from(*x))
        .sum();
    let num = 2 * weighted * 1_000_000;
    let den = n * total;
    let g = num / den;
    let sub = (n + 1) * 1_000_000 / n;
    u64::try_from(g.saturating_sub(sub)).unwrap_or(1_000_000)
}

/// Stakes after capping each at `cap_ppm` of the total, with the excess
/// redistributed pro rata to delegations below the cap — the effect a stake
/// cap would have if delegators moved rather than left. Simulated before any
/// cap is proposed (Master Prompt 18 §3).
pub fn apply_cap(stakes: &[u64], cap_ppm: u64) -> Vec<u64> {
    let total: u128 = stakes.iter().map(|s| u128::from(*s)).sum();
    let cap = u64::try_from(total * u128::from(cap_ppm) / 1_000_000).unwrap_or(u64::MAX);
    let mut out: Vec<u64> = stakes.to_vec();
    for _ in 0..64 {
        let excess: u128 = out.iter().map(|s| u128::from(s.saturating_sub(cap))).sum();
        if excess == 0 {
            break;
        }
        let below: u128 = out.iter().filter(|s| **s < cap).map(|s| u128::from(*s)).sum();
        if below == 0 {
            break;
        }
        for s in &mut out {
            if *s >= cap {
                *s = cap;
            } else {
                let add = u128::from(*s) * excess / below;
                *s = s.saturating_add(u64::try_from(add).unwrap_or(0)).min(cap);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_stakes_have_zero_gini_and_a_third_plus_one_nakamoto() {
        let equal = vec![100u64; 9];
        assert_eq!(gini_ppm(&equal), 0);
        assert_eq!(nakamoto_coefficient(&equal, 333_334), 4);
    }

    #[test]
    fn one_whale_dominates() {
        let mut s = vec![1u64; 99];
        s.push(1_000_000);
        assert_eq!(nakamoto_coefficient(&s, 333_334), 1);
        assert!(gini_ppm(&s) > 980_000);
    }

    #[test]
    fn a_cap_raises_the_nakamoto_coefficient() {
        let mut s: Vec<u64> = (1..=50).map(|i| i * i).collect();
        s.push(40_000);
        let before = nakamoto_coefficient(&s, 333_334);
        let after = nakamoto_coefficient(&apply_cap(&s, 50_000), 333_334);
        assert!(after > before, "{before} -> {after}");
    }
}
