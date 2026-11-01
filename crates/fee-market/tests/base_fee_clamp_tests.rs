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

/// Reads a file from the repository root (this crate sits two levels down).
fn repo_file(relative: &str) -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
    std::fs::read_to_string(format!("{path}{relative}"))
        .unwrap_or_else(|e| panic!("reading {relative}: {e}"))
}

/// The digits of `text` with `_` separators removed, parsed.
fn number(text: &str) -> u128 {
    let digits: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("no number in {text:?}"))
}

#[test]
fn the_conformance_vectors_carry_the_compiled_in_ceiling() {
    // Drift guard: spec/tests/fees.json is generated from spec-ref, which
    // mirrors this constant. A changed limits.rs that is not regenerated fails here.
    let ceiling = MAX_BASE_FEE_PER_BYTE.to_u128().unwrap();
    let fields: Vec<u128> = repo_file("spec/tests/fees.json")
        .lines()
        .filter_map(|line| line.trim().strip_prefix("\"ceiling\": \""))
        .map(|rest| number(rest.trim_end_matches([',', '"'])))
        .collect();
    assert!(
        !fields.is_empty(),
        "no ceiling fields in spec/tests/fees.json"
    );
    for value in fields {
        assert_eq!(value, ceiling, "spec/tests/fees.json ceiling drifted");
    }
}

#[test]
fn the_reference_implementation_mirrors_the_ceiling() {
    let line = repo_file("crates/spec-ref/src/fees.rs")
        .lines()
        .find(|l| l.contains("pub const MAX_BASE_FEE_PER_BYTE: u64 ="))
        .map(str::to_owned)
        .expect("spec-ref declares MAX_BASE_FEE_PER_BYTE");
    let value = number(line.split('=').nth(1).expect("an assignment"));
    assert_eq!(
        value,
        MAX_BASE_FEE_PER_BYTE.to_u128().unwrap(),
        "spec-ref drifted"
    );
}

#[test]
fn the_z3_model_mirrors_the_ceiling() {
    let line = repo_file("formal/z3/fee_market.py")
        .lines()
        .find(|l| l.starts_with("MAX_BASE_FEE_PER_BYTE ="))
        .map(str::to_owned)
        .expect("the Z3 model declares MAX_BASE_FEE_PER_BYTE");
    let value = number(
        line.split('=')
            .nth(1)
            .expect("an assignment")
            .split('#')
            .next()
            .unwrap(),
    );
    assert_eq!(
        value,
        MAX_BASE_FEE_PER_BYTE.to_u128().unwrap(),
        "Z3 model drifted"
    );
}
