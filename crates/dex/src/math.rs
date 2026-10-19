//! Wide-intermediate integer arithmetic.
//!
//! Every amount is 256-bit (ADR-046), and every product formed here is the
//! 512-bit [`Wide`] of two of them, which cannot overflow. Computing wide and
//! reducing back is what makes the reserve arithmetic exact instead of merely
//! usually-correct: the naive `reserve_out * amount_in / (reserve_in +
//! amount_in)` at the amounts' own width wraps for any pool past half the
//! width on each side.
//!
//! The two rounding directions are separate functions rather than a flag,
//! because which one a call site wants is a *safety* property. A payout that
//! rounds up and a charge that rounds down are the same bug — value created
//! from nothing — and naming them apart makes that visible at the call site.

use maya_amount::{Amount, Wide};

/// `(a * b) / c`, rounded down.
///
/// Returns `None` if `c` is zero or the quotient does not fit in an
/// [`Amount`]. The product is 512-bit, so it never overflows.
#[must_use]
pub const fn mul_div_floor(a: Amount, b: Amount, c: Amount) -> Option<Amount> {
    a.mul_div(b, c)
}

/// `(a * b) / c`, rounded up.
///
/// Returns `None` if `c` is zero or the quotient does not fit in an
/// [`Amount`].
#[must_use]
pub const fn mul_div_ceil(a: Amount, b: Amount, c: Amount) -> Option<Amount> {
    let Some((quotient, remainder)) = a.mul_wide(b).checked_div_rem(c) else {
        return None;
    };
    if remainder.is_zero() {
        Some(quotient)
    } else {
        quotient.checked_add(Amount::ONE)
    }
}

/// A basis-point or other small `u32` figure as an [`Amount`].
#[must_use]
pub const fn small(value: u32) -> Amount {
    Amount::from_u64(value as u64)
}

/// Integer square root of a 512-bit value, rounded down.
///
/// Used once, for the initial liquidity of a pool: the first depositor's share
/// count is the geometric mean of the two reserves, which is the only quantity
/// invariant to the pair's ordering and to the units either side is denominated
/// in. The root of anything below 2^512 is below 2^256, so it always fits.
///
/// Bit by bit from the top: 256 squarings, each one 512-bit product. It runs
/// once per pool, never in a per-transaction loop.
#[must_use]
pub fn isqrt_wide(value: Wide) -> Amount {
    let mut root = Amount::ZERO;
    for bit in (0..256u32).rev() {
        let mut limbs = root.0;
        limbs[(bit / 64) as usize] |= 1u64 << (bit % 64);
        let candidate = Amount(limbs);
        if candidate.mul_wide(candidate) <= value {
            root = candidate;
        }
    }
    root
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn a(v: u64) -> Amount {
        Amount::from_u64(v)
    }

    #[test]
    fn floor_and_ceil_agree_on_exact_division() {
        assert_eq!(mul_div_floor(a(6), a(7), a(21)), Some(a(2)));
        assert_eq!(mul_div_ceil(a(6), a(7), a(21)), Some(a(2)));
    }

    #[test]
    fn ceil_rounds_away_from_zero_on_a_remainder() {
        assert_eq!(mul_div_floor(a(10), a(1), a(3)), Some(a(3)));
        assert_eq!(mul_div_ceil(a(10), a(1), a(3)), Some(a(4)));
    }

    #[test]
    fn division_by_zero_is_none_rather_than_a_panic() {
        assert_eq!(mul_div_floor(a(1), a(1), Amount::ZERO), None);
        assert_eq!(mul_div_ceil(a(1), a(1), Amount::ZERO), None);
    }

    #[test]
    fn products_of_two_maxima_do_not_overflow_the_intermediate() {
        // Would wrap in 256 bits; the 512-bit intermediate gets it right.
        let big = Amount::MAX;
        assert_eq!(mul_div_floor(big, big, big), Some(big));
        assert_eq!(mul_div_ceil(big, big, big), Some(big));
    }

    #[test]
    fn a_quotient_too_large_is_rejected_not_truncated() {
        assert_eq!(mul_div_floor(Amount::MAX, a(2), a(1)), None);
        // MAX * MAX / (MAX - 1) leaves a remainder; rounding up would pass MAX.
        assert_eq!(mul_div_ceil(Amount::MAX, a(1), a(1)), Some(Amount::MAX));
    }

    #[test]
    fn isqrt_is_exact_on_squares_and_floors_otherwise() {
        let root = |v: u64| isqrt_wide(a(v).mul_wide(Amount::ONE));
        assert_eq!(root(0), Amount::ZERO);
        assert_eq!(root(1), a(1));
        assert_eq!(root(15), a(3));
        assert_eq!(root(16), a(4));
        assert_eq!(isqrt_wide(Amount::MAX.mul_wide(Amount::MAX)), Amount::MAX);
        let big = a(u64::MAX);
        assert_eq!(isqrt_wide(big.mul_wide(big)), big);
        // One below a perfect square floors to the smaller root.
        let below = Amount::MAX.mul_wide(Amount::MAX);
        let just_under = isqrt_wide(Amount::MAX.checked_sub(a(1)).unwrap().mul_wide(Amount::MAX));
        assert!(just_under < Amount::MAX && isqrt_wide(below) == Amount::MAX);
    }
}
