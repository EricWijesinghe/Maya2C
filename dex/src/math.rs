//! Wide-intermediate integer arithmetic.
//!
//! Every product formed here is `u64 * u64`, which overflows `u64` and does not
//! overflow `u128`. Computing in `u128` and reducing back is what makes the
//! reserve arithmetic exact instead of merely usually-correct: the naive
//! `reserve_out * amount_in / (reserve_in + amount_in)` in `u64` wraps for any
//! pool holding more than about four billion units on each side, which is not a
//! large pool.
//!
//! The two rounding directions are separate functions rather than a flag,
//! because which one a call site wants is a *safety* property. A payout that
//! rounds up and a charge that rounds down are the same bug — value created
//! from nothing — and naming them apart makes that visible at the call site.

/// `(a * b) / c`, rounded down.
///
/// Returns `None` if `c` is zero or the quotient does not fit in `u64`.
#[must_use]
pub const fn mul_div_floor(a: u128, b: u128, c: u128) -> Option<u64> {
    if c == 0 {
        return None;
    }
    // `a * b` can overflow `u128` for sufficiently large inputs. Every caller
    // supplies `u64`-derived values, for which it cannot, but the check is here
    // rather than in a comment because a future caller will not read the
    // comment.
    let Some(product) = a.checked_mul(b) else {
        return None;
    };
    narrow(product / c)
}

/// `(a * b) / c`, rounded up.
///
/// Returns `None` if `c` is zero or the quotient does not fit in `u64`.
#[must_use]
pub const fn mul_div_ceil(a: u128, b: u128, c: u128) -> Option<u64> {
    if c == 0 {
        return None;
    }
    let Some(product) = a.checked_mul(b) else {
        return None;
    };
    // `product + c - 1` cannot overflow for any product a `u64`-derived call
    // site can form, but the checked form costs nothing and removes the need to
    // trust that.
    let Some(biased) = product.checked_add(c - 1) else {
        return None;
    };
    narrow(biased / c)
}

/// Narrows a `u128` to `u64`, or `None` if it does not fit.
#[must_use]
pub const fn narrow(value: u128) -> Option<u64> {
    if value > u64::MAX as u128 {
        None
    } else {
        Some(value as u64)
    }
}

/// Integer square root of a `u128`, narrowed to `u64`.
///
/// Used once, for the initial liquidity of a pool: the first depositor's share
/// count is the geometric mean of the two reserves, which is the only quantity
/// invariant to the pair's ordering and to the units either side is denominated
/// in.
///
/// Returns `None` only if the root does not fit in `u64`, which requires a
/// product above `2^128` and is therefore unreachable from two `u64` reserves.
#[must_use]
pub const fn isqrt_u128(value: u128) -> Option<u64> {
    narrow(value.isqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_and_ceil_agree_on_exact_division() {
        assert_eq!(mul_div_floor(6, 7, 21), Some(2));
        assert_eq!(mul_div_ceil(6, 7, 21), Some(2));
    }

    #[test]
    fn ceil_rounds_away_from_zero_on_a_remainder() {
        assert_eq!(mul_div_floor(10, 1, 3), Some(3));
        assert_eq!(mul_div_ceil(10, 1, 3), Some(4));
    }

    #[test]
    fn division_by_zero_is_none_rather_than_a_panic() {
        assert_eq!(mul_div_floor(1, 1, 0), None);
        assert_eq!(mul_div_ceil(1, 1, 0), None);
    }

    #[test]
    fn products_of_two_u64_maxima_do_not_overflow_the_intermediate() {
        let big = u64::MAX as u128;
        // Would wrap to 1 in `u64`; the wide intermediate gets it right.
        assert_eq!(mul_div_floor(big, big, big), Some(u64::MAX));
    }

    #[test]
    fn a_quotient_too_large_for_u64_is_rejected_not_truncated() {
        let big = u64::MAX as u128;
        assert_eq!(mul_div_floor(big, big, 1), None);
    }

    #[test]
    fn isqrt_is_exact_on_squares_and_floors_otherwise() {
        assert_eq!(isqrt_u128(0), Some(0));
        assert_eq!(isqrt_u128(1), Some(1));
        assert_eq!(isqrt_u128(15), Some(3));
        assert_eq!(isqrt_u128(16), Some(4));
        let big = u64::MAX as u128;
        assert_eq!(isqrt_u128(big * big), Some(u64::MAX));
    }
}
