//! Per-operator cap on committee voting weight (CON-06, Eric 2026-11-24).
//!
//! Consensus-critical: integers only, every map a `BTreeMap`, ties by
//! address, so every node computes the same weights (invariant 24).

use std::collections::BTreeMap;

/// Most of a committee's voting weight one operator may hold: 20% (CON-06,
/// Eric 2026-11-24), below the third that could stop finality.
pub const MAX_OPERATOR_WEIGHT_BPS: u64 = 2_000;

const BPS: u64 = 10_000;

/// Caps every operator's summed weight at [`MAX_OPERATOR_WEIGHT_BPS`] of the
/// capped total, by water-filling in integers: operators ordered by weight
/// (ties by address), the `k` heaviest are cut to
/// `c = cap × rest ÷ (BPS − cap × k)`, rounded down, for the least `k` at
/// which the next operator is at or below `c` (`rest` is the weight of all but
/// the `k`). Then `c ≤ cap × (k·c + rest) ÷ BPS`, so every operator holds at
/// most the cap of the new total. A cut operator's members are scaled by
/// `c ÷ its weight`, rounded down, keeping at least one, so the bound holds to
/// within one unit of weight per key (one up for each of the operator's own,
/// one down for each in the committee). Weights are stakes in base units, so
/// that unit is far below one coin.
///
/// With fewer than `BPS ÷ cap` = 5 operators no weights can satisfy the cap,
/// so they are returned unchanged; a value-bearing genesis seats at least
/// seven validators (`genesis.rs`). Operator is the address the stake pays
/// out to, so one operator running many keys is one operator here.
#[must_use]
pub fn cap_operator_weights(weights: &[u64], operators: &[[u8; 32]]) -> Vec<u64> {
    let mut totals: BTreeMap<[u8; 32], u128> = BTreeMap::new();
    for (w, op) in weights.iter().zip(operators) {
        *totals.entry(*op).or_insert(0) += u128::from(*w);
    }
    let mut ranked: Vec<([u8; 32], u128)> = totals.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let cap = u128::from(MAX_OPERATOR_WEIGHT_BPS);
    let groups = u128::from(BPS / MAX_OPERATOR_WEIGHT_BPS);
    let mut rest: u128 = ranked.iter().map(|(_, t)| t).sum();
    let mut level = None;
    for (k, (_, t)) in ranked.iter().enumerate() {
        let k = k as u128;
        if k >= groups {
            break;
        }
        let c = cap * rest / (u128::from(BPS) - cap * k);
        if *t <= c {
            level = Some(c);
            break;
        }
        rest -= t;
    }
    let Some(level) = level else {
        return weights.to_vec();
    };
    let cut: BTreeMap<[u8; 32], u128> = ranked.into_iter().filter(|(_, t)| *t > level).collect();
    weights
        .iter()
        .zip(operators)
        .map(|(w, op)| match cut.get(op) {
            // Below `w` because `level < total`, so it fits a u64.
            Some(total) if *w > 0 => {
                u64::try_from(u128::from(*w) * level / total).map_or(*w, |v| v.max(1))
            }
            _ => *w,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn op(n: u8) -> [u8; 32] {
        [n; 32]
    }

    /// One operator's summed weight and the capped total.
    fn held(weights: &[u64], operators: &[[u8; 32]], who: [u8; 32]) -> (u128, u128) {
        let total = weights.iter().map(|w| u128::from(*w)).sum();
        let mine = weights
            .iter()
            .zip(operators)
            .filter(|(_, o)| **o == who)
            .map(|(w, _)| u128::from(*w))
            .sum();
        (mine, total)
    }

    #[test]
    fn equal_bonds_are_left_exactly_as_they_are() {
        let ops: Vec<_> = (1..=7).map(op).collect();
        assert_eq!(cap_operator_weights(&[1_000; 7], &ops), vec![1_000; 7]);
    }

    #[test]
    fn a_whale_is_cut_to_a_fifth_of_the_new_total_and_nobody_else_moves() {
        let ops: Vec<_> = (1..=7).map(op).collect();
        let mut w = vec![1_000; 7];
        w[0] = 10_000;
        let capped = cap_operator_weights(&w, &ops);
        // c = 2000 x 6000 / (10000 - 2000) = 1500, and 1500 / 7500 = 20%.
        assert_eq!(capped[0], 1_500);
        assert_eq!(&capped[1..], &w[1..]);
    }

    #[test]
    fn one_operator_running_many_keys_is_capped_as_one() {
        // Operator 1 runs three of seven keys: 3/7 of the weight uncapped.
        let ops = [op(1), op(1), op(1), op(2), op(3), op(4), op(5)];
        let capped = cap_operator_weights(&[1_000; 7], &ops);
        let (mine, total) = held(&capped, &ops, op(1));
        assert!(mine * u128::from(BPS) <= u128::from(MAX_OPERATOR_WEIGHT_BPS) * total);
        assert_eq!(capped[0], capped[1], "its keys are scaled alike");
        assert_eq!(&capped[3..], &[1_000; 4]);
    }

    #[test]
    fn below_five_operators_no_cap_is_satisfiable_so_none_is_applied() {
        let ops: Vec<_> = (1..=4).map(op).collect();
        let w = [9_000, 9_000, 10, 10];
        assert_eq!(cap_operator_weights(&w, &ops), w.to_vec());
    }

    /// Fixed-seed sweep: 5 to 40 operators, one to three keys each, stakes
    /// over six orders of magnitude, down to single units where rounding
    /// binds. Every operator ends at or below the cap to within one unit per
    /// key; no weight grows; nobody with stake loses all weight.
    #[test]
    fn no_operator_ever_holds_more_than_the_cap() {
        let mut seed: u64 = 0x5eed_2026_1124;
        let mut next = |n: u64| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % n
        };
        for _ in 0..2_000 {
            let n_ops = 5 + next(36);
            let mut weights = Vec::new();
            let mut operators = Vec::new();
            for o in 0..n_ops {
                for _ in 0..=next(3) {
                    let scale = 10u64.pow(1 + u32::try_from(next(6)).unwrap());
                    weights.push(1 + next(scale));
                    operators.push(op(u8::try_from(o).unwrap()));
                }
            }
            let capped = cap_operator_weights(&weights, &operators);
            for o in 0..n_ops {
                let who = op(u8::try_from(o).unwrap());
                let (mine, total) = held(&capped, &operators, who);
                // Rounding: each of its keys may sit one unit above its exact
                // share (the floor of one), and each key in the committee one
                // unit below (rounding down), so the bound is exact to that.
                let own = operators.iter().filter(|x| **x == who).count() as u128;
                let keys = operators.len() as u128;
                assert!(
                    mine.saturating_sub(own) * u128::from(BPS)
                        <= u128::from(MAX_OPERATOR_WEIGHT_BPS) * (total + keys),
                    "operator {o} holds {mine} of {total}"
                );
            }
            assert!(capped.iter().all(|w| *w > 0));
            assert!(capped.iter().zip(&weights).all(|(c, w)| c <= w));
        }
    }
}
