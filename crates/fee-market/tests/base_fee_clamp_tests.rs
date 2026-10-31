//! The base-fee step clamp and ceiling (ADR-071).
//!
//! Before ADR-071 a block at 64 times the target multiplied the base fee by
//! about 8.9, and nothing bounded it. These tests pin the three properties
//! that now hold: the rise per block is at most `1/denominator`, the fee never
//! exceeds `MAX_BASE_FEE_PER_BYTE`, and a one-coin account can still pay for a
//! standard transfer after a full-block attack.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_fee_market::limits::{MAX_BASE_FEE_PER_BYTE, MIN_BASE_FEE_FLOOR};
use maya_fee_market::{Amount, ConfigError, FeeConfig, next_base_fee};

const TARGET: u64 = 1_000_000;
const DENOM: u64 = 8;
/// Bytes in one signed standard transfer (`reports/13-pq-weight.md`: 13,215).
const STANDARD_TRANSFER_BYTES: u64 = 13_215;
/// One coin at 24 decimals (`apps/wallet-gui/core/src/amount.rs::DECIMALS`).
const ONE_COIN: u128 = 1_000_000_000_000_000_000_000_000;

fn a(n: u64) -> Amount {
    Amount::from_u64(n)
}

/// One block of `size` bytes against the module's target and denominator.
fn step(fee: Amount, size: u64) -> Amount {
    next_base_fee(fee, size, TARGET, DENOM, MIN_BASE_FEE_FLOOR)
}

#[test]
fn twenty_blocks_at_sixty_four_times_target_rise_no_faster_than_one_eighth_each() {
    // Each step is at most fee + fee/8, which keeps the integer fee at or
    // below floor(9^n * f0 / 8^n). Before the clamp the same run reaches about
    // 8.9^20 times the start.
    let start = 1_000u128;
    let mut fee = a(start as u64);
    for _ in 0..20 {
        fee = step(fee, 64 * TARGET);
    }
    let bound = start * 9u128.pow(20) / 8u128.pow(20);
    assert!(
        fee <= Amount::from_u128(bound),
        "after 20 full blocks fee is {fee}, bound {bound}"
    );
}

#[test]
fn a_single_block_never_raises_the_fee_by_more_than_one_denominator_share() {
    for fee in [a(1), a(7), a(8), a(1_000), a(123_456_789)] {
        let next = step(fee, u64::MAX);
        let share = fee.checked_div(a(DENOM)).unwrap().max(a(1));
        assert!(
            next <= fee.saturating_add(share),
            "fee {fee} rose to {next}"
        );
    }
}

#[test]
fn the_ceiling_is_never_exceeded_under_sustained_full_blocks() {
    let mut fee = a(10);
    let mut reached = false;
    for _ in 0..600 {
        fee = step(fee, 64 * TARGET);
        assert!(fee <= MAX_BASE_FEE_PER_BYTE, "fee {fee} above the ceiling");
        reached |= fee == MAX_BASE_FEE_PER_BYTE;
    }
    assert!(reached, "600 full blocks should reach the ceiling");
}

#[test]
fn a_fee_above_the_ceiling_holds_and_does_not_fall_on_a_full_block() {
    let above = MAX_BASE_FEE_PER_BYTE.saturating_add(a(1_000));
    assert_eq!(step(above, 64 * TARGET), above);
}

#[test]
fn a_one_coin_account_pays_a_standard_transfer_after_a_max_fill_attack() {
    let mut fee = a(10);
    for _ in 0..600 {
        fee = step(fee, 64 * TARGET);
    }
    let required = fee.checked_mul(a(STANDARD_TRANSFER_BYTES)).unwrap();
    assert!(
        required <= Amount::from_u128(ONE_COIN),
        "a standard transfer costs {required} at the ceiling, one coin is {ONE_COIN}"
    );
}

#[test]
fn the_ceiling_is_a_configuration_bound() {
    let mut config = FeeConfig::TESTING;
    config.initial_base_fee = MAX_BASE_FEE_PER_BYTE.saturating_add(a(1));
    assert!(matches!(config.validate(), Err(ConfigError::Ceiling(_))));
    config.initial_base_fee = MAX_BASE_FEE_PER_BYTE;
    assert!(config.validate().is_ok());
}
