//! `Amount` against `u128` where values fit, and against its own algebra
//! where they do not: the long-division path (divisors above 2^64), which
//! the Kani harnesses bound away, is checked here.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_amount::{Amount, Wide};
use proptest::prelude::*;

fn amount() -> impl Strategy<Value = Amount> {
    any::<[u64; 4]>().prop_map(Amount)
}

/// Amounts of every size, not just huge ones: a uniform [u64; 4] is almost
/// always near 2^256.
fn any_width() -> impl Strategy<Value = Amount> {
    (amount(), 0usize..=4).prop_map(|(a, keep)| {
        let mut l = a.0;
        for limb in l.iter_mut().skip(keep) {
            *limb = 0;
        }
        Amount(l)
    })
}

proptest! {
    #[test]
    fn add_sub_mul_match_u128(x in any::<u64>(), y in any::<u64>()) {
        let (a, b) = (Amount::from(x), Amount::from(y));
        prop_assert_eq!(a.checked_add(b), Some(Amount::from(u128::from(x) + u128::from(y))));
        prop_assert_eq!(a.checked_sub(b), x.checked_sub(y).map(Amount::from));
        prop_assert_eq!(a.checked_mul(b), Some(Amount::from(u128::from(x) * u128::from(y))));
        prop_assert_eq!(a.cmp(&b), x.cmp(&y));
    }

    #[test]
    fn division_matches_u128(n in any::<u128>(), d in 1u128..) {
        let (q, r) = Wide::from_amount(Amount::from(n)).checked_div_rem(Amount::from(d)).unwrap();
        prop_assert_eq!(q, Amount::from(n / d));
        prop_assert_eq!(r, Amount::from(n % d));
    }

    #[test]
    fn every_division_satisfies_q_d_plus_r(n in any_width(), m in any_width(), d in any_width()) {
        prop_assume!(!d.is_zero());
        let w = n.mul_wide(m);
        match w.checked_div_rem(d) {
            Some((q, r)) => {
                prop_assert!(Amount([w.0[4], w.0[5], w.0[6], w.0[7]]) < d);
                prop_assert!(r < d);
                // q·d + r == n·m, compared in 512 bits.
                let back = q.mul_wide(d);
                let sum = add_wide(back, Wide::from_amount(r));
                prop_assert_eq!(sum, w);
            }
            // The quotient is 2^256 or more exactly when the top 256 bits of
            // n·m are at least d; refused in that case and no other.
            None => prop_assert!(Amount([w.0[4], w.0[5], w.0[6], w.0[7]]) >= d),
        }
    }

    #[test]
    fn mul_div_round_trips(a in any_width(), d in any_width()) {
        prop_assume!(!d.is_zero());
        prop_assert_eq!(a.mul_div(d, d), Some(a));
    }

    #[test]
    fn decimal_text_matches_u128(x in any::<u128>()) {
        prop_assert_eq!(Amount::from(x).to_string(), x.to_string());
    }

    #[test]
    fn bytes_round_trip(a in amount()) {
        prop_assert_eq!(Amount::from_le_bytes(a.to_le_bytes()), a);
    }
}

/// 512-bit add for the test's own check; Wide has none on purpose.
fn add_wide(a: Wide, b: Wide) -> Wide {
    let mut out = [0u64; 8];
    let mut carry = false;
    for ((slot, x), y) in out.iter_mut().zip(a.0).zip(b.0) {
        let (s1, c1) = x.overflowing_add(y);
        let (s2, c2) = s1.overflowing_add(u64::from(carry));
        *slot = s2;
        carry = c1 || c2;
    }
    assert!(!carry, "q·d + r cannot exceed n·m");
    Wide(out)
}
