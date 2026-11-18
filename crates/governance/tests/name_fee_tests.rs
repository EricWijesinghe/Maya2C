//! The name-fee defaults Eric set on 2026-11-12 (ADR-135 O-3): 1 coin a year
//! for a 3-byte label, 0.1 for 4, 0.001 for 5 and longer, all governed.
//!
//! The defaults are written out here in base units rather than recomputed from
//! the constants, so a change to the constants that drifts from the decision
//! fails this file.

use maya_amount::{Amount, ONE_COIN};
use maya_governance::params::{
    NAME_BASE_FEE_3, NAME_BASE_FEE_4, NAME_BASE_FEE_5, ParameterChange, ParameterKey,
};

const NAME_KEYS: [ParameterKey; 3] = [
    ParameterKey::NameBaseFee3,
    ParameterKey::NameBaseFee4,
    ParameterKey::NameBaseFee5,
];

#[test]
fn one_coin_is_ten_to_the_twenty_four_base_units() {
    assert_eq!(ONE_COIN, 1_000_000_000_000_000_000_000_000);
}

#[test]
fn each_name_fee_default_is_exactly_one_tenth_and_a_thousandth_of_a_coin() {
    let one = Amount::from_u128(1_000_000_000_000_000_000_000_000);
    let tenth = Amount::from_u128(100_000_000_000_000_000_000_000);
    let thousandth = Amount::from_u128(1_000_000_000_000_000_000_000);

    assert_eq!(ParameterKey::NameBaseFee3.bounds().default, one);
    assert_eq!(ParameterKey::NameBaseFee4.bounds().default, tenth);
    assert_eq!(ParameterKey::NameBaseFee5.bounds().default, thousandth);
    assert_eq!(NAME_BASE_FEE_3, one);
    assert_eq!(NAME_BASE_FEE_4, tenth);
    assert_eq!(NAME_BASE_FEE_5, thousandth);
}

#[test]
fn the_long_name_fee_is_one_hundredth_of_the_four_byte_fee_and_not_zero() {
    let four = ONE_COIN / 10;
    assert_eq!(NAME_BASE_FEE_5, Amount::from_u128(four / 100));
    assert_ne!(
        NAME_BASE_FEE_5,
        Amount::from_u64(0),
        "a long name is not free"
    );
}

#[test]
fn every_name_fee_default_is_inside_its_governed_range() {
    for key in NAME_KEYS {
        let bounds = key.bounds();
        assert!(key.check(bounds.default).is_ok(), "{}", key.label());
        // Headroom: governance can raise the fee a millionfold. Division,
        // not a multiply, so no 512-bit intermediate (Eric, 2026-11-01).
        let millionth_of_max = bounds
            .max
            .checked_div(Amount::from_u64(1_000_000))
            .expect("non-zero divisor");
        assert!(
            bounds.default <= millionth_of_max,
            "{} has no headroom",
            key.label()
        );
    }
}

#[test]
fn a_name_fee_proposal_above_the_max_is_refused() {
    for key in NAME_KEYS {
        let max = key.bounds().max;
        let at_max = ParameterChange { key, value: max };
        assert!(at_max.validate().is_ok(), "{} at max", key.label());
        let above = ParameterChange {
            key,
            value: max
                .checked_add(Amount::from_u64(1))
                .expect("below U256::MAX"),
        };
        assert!(above.validate().is_err(), "{} above max", key.label());
        let top = ParameterChange {
            key,
            value: Amount::MAX,
        };
        assert!(top.validate().is_err(), "{} at U256::MAX", key.label());
    }
}

#[test]
fn ten_periods_of_the_largest_legal_fee_fit_in_256_bits() {
    // The node multiplies the fee by `periods` (1 to 10) with one checked
    // U256 multiply; the governed ceiling must keep that from overflowing.
    let tenth_of_u256 = Amount::MAX
        .checked_div(Amount::from_u64(10))
        .expect("non-zero divisor");
    for key in NAME_KEYS {
        assert!(key.bounds().max <= tenth_of_u256, "{}", key.label());
    }
}
