#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use std::string::ToString;

/// 2^256 − 1, from any big-number tool.
const MAX_DEC: &str =
    "115792089237316195423570985008687907853269984665640564039457584007913129639935";

fn a(n: u64) -> Amount {
    Amount::from(n)
}

#[test]
fn max_and_zero_print_exactly() {
    assert_eq!(Amount::MAX.to_string(), MAX_DEC);
    assert_eq!(Amount::ZERO.to_string(), "0");
    assert_eq!(a(u64::MAX).to_string(), "18446744073709551615");
    // A chunk boundary: the zero padding inside must survive.
    assert_eq!(Amount([0, 1, 0, 0]).to_string(), "18446744073709551616");
    assert_eq!(
        a(10_000_000_000_000_000_000).to_string(),
        "10000000000000000000"
    );
}

#[test]
fn add_carries_across_every_limb_and_stops_at_max() {
    let low_full = Amount([u64::MAX, u64::MAX, u64::MAX, 0]);
    assert_eq!(
        low_full.checked_add(Amount::ONE),
        Some(Amount([0, 0, 0, 1]))
    );
    assert_eq!(Amount::MAX.checked_add(Amount::ONE), None);
    assert_eq!(Amount::MAX.saturating_add(Amount::ONE), Amount::MAX);
}

#[test]
fn sub_borrows_across_every_limb_and_stops_at_zero() {
    assert_eq!(
        Amount([0, 0, 0, 1]).checked_sub(Amount::ONE),
        Some(Amount([u64::MAX, u64::MAX, u64::MAX, 0]))
    );
    assert_eq!(Amount::ZERO.checked_sub(Amount::ONE), None);
    assert_eq!(Amount::ZERO.saturating_sub(Amount::ONE), Amount::ZERO);
}

#[test]
fn order_is_numeric_not_limbwise() {
    let high = Amount([0, 0, 0, 1]);
    let low = Amount([u64::MAX, 0, 0, 0]);
    assert!(high > low);
    assert!(Amount::MAX > high);
    assert!(Amount::ZERO < Amount::ONE);
}

#[test]
fn mul_wide_of_max_is_two_to_the_512_minus_two_to_the_257_plus_one() {
    // (2^256 − 1)^2 = 2^512 − 2^257 + 1: limbs [1, 0, 0, 0, MAX−1, MAX, MAX, MAX].
    let w = Amount::MAX.mul_wide(Amount::MAX);
    assert_eq!(
        w.0,
        [1, 0, 0, 0, u64::MAX - 1, u64::MAX, u64::MAX, u64::MAX]
    );
    assert_eq!(Amount::MAX.checked_mul(Amount::MAX), None);
    assert_eq!(Amount::MAX.checked_mul(Amount::ONE), Some(Amount::MAX));
}

#[test]
fn mul_div_keeps_a_product_that_would_overflow() {
    // MAX × MAX / MAX = MAX, though MAX × MAX needs 512 bits.
    assert_eq!(
        Amount::MAX.mul_div(Amount::MAX, Amount::MAX),
        Some(Amount::MAX)
    );
    // A quotient past 2^256 is refused, not truncated.
    assert_eq!(Amount::MAX.mul_div(Amount::MAX, a(2)), None);
    assert_eq!(a(7).mul_div(a(3), Amount::ZERO), None);
    assert_eq!(a(7).mul_div(a(3), a(2)), Some(a(10)));
}

#[test]
fn long_division_matches_the_small_divisor_path() {
    // A divisor above 2^64 takes long_div; check it against a known answer.
    let d = Amount([0, 1, 0, 0]); // 2^64
    let n = Amount([5, 7, 9, 11]);
    let (q, r) = Wide::from_amount(n).checked_div_rem(d).unwrap();
    assert_eq!(q, Amount([7, 9, 11, 0]));
    assert_eq!(r, a(5));
    assert_eq!(Amount::MAX.checked_div(Amount::MAX), Some(Amount::ONE));
    assert_eq!(a(1).checked_div(Amount::MAX), Some(Amount::ZERO));
}

#[test]
fn bytes_round_trip_low_byte_first() {
    let v = Amount([1, 2, 3, 4]);
    let bytes = v.to_le_bytes();
    assert_eq!(bytes[0], 1);
    assert_eq!(bytes[8], 2);
    assert_eq!(bytes[24], 4);
    assert_eq!(Amount::from_le_bytes(bytes), v);
    assert_eq!(Amount::from_le_bytes([0xff; BYTES]), Amount::MAX);
}

#[test]
fn narrowing_is_exact_or_refused() {
    assert_eq!(a(7).to_u64(), Some(7));
    assert_eq!(Amount([0, 1, 0, 0]).to_u64(), None);
    assert_eq!(Amount([0, 1, 0, 0]).to_u128(), Some(1u128 << 64));
    assert_eq!(Amount([0, 0, 1, 0]).to_u128(), None);
    assert_eq!(Amount::MAX.saturating_to_u64(), u64::MAX);
    assert_eq!(Amount::from(u128::MAX).to_u128(), Some(u128::MAX));
    assert!(Amount::ZERO.is_zero() && !Amount::ONE.is_zero());
}

#[test]
fn bits_counts_the_highest_set_bit() {
    assert_eq!(Amount::ZERO.bits(), 0);
    assert_eq!(Amount::ONE.bits(), 1);
    assert_eq!(a(u64::MAX).bits(), 64);
    assert_eq!(Amount([0, 1, 0, 0]).bits(), 65);
    assert_eq!(Amount::MAX.bits(), 256);
}

#[test]
fn shr_moves_bits_across_limbs_and_drops_the_rest() {
    assert_eq!(Amount([0, 1, 0, 0]).shr(1), a(1 << 63));
    assert_eq!(Amount([0, 0, 0, 1]).shr(192), Amount::ONE);
    assert_eq!(Amount::MAX.shr(255), Amount::ONE);
    assert_eq!(Amount::MAX.shr(256), Amount::ZERO);
    assert_eq!(Amount::MAX.shr(0), Amount::MAX);
    assert_eq!(Amount::MAX.shr(192), a(u64::MAX));
    assert_eq!(a(6).shr(1), a(3));
}

#[test]
fn wide_order_is_numeric_not_limbwise() {
    let high = Amount::MAX.mul_wide(Amount::from_u64(2));
    let low = Amount::MAX.mul_wide(Amount::ONE);
    assert!(high > low);
    assert!(Wide::from_amount(Amount([0, 0, 0, 1])) > Wide::from_amount(Amount([u64::MAX, u64::MAX, u64::MAX, 0])));
    assert_eq!(low.cmp(&low), core::cmp::Ordering::Equal);
}
