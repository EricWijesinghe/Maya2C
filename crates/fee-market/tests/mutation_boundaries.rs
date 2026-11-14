//! Tests written from `cargo mutants` survivors (reports/08-security.md).
//!
//! Each one pins a boundary a mutant moved without any test noticing:
//! `>` vs `>=` in validation, `*` vs `+` in the limit constants, the exact-fee
//! case of the underpricing check. The neural model's tests moved with it to
//! `crates/neural-fee/tests/mutation_boundaries.rs` (2026-11-14).

#![allow(clippy::unwrap_used)]

use maya_fee_market::limits::{MAX_TARGET_BLOCK_BYTES, MAX_TREASURY_BPS, MIN_TARGET_BLOCK_BYTES};
use maya_fee_market::{ConfigError, FeeClaim, FeeConfig, TxFee, apply_block_fees};

#[test]
fn limits_are_the_documented_sizes() {
    assert_eq!(MIN_TARGET_BLOCK_BYTES, 65_536);
    assert_eq!(MAX_TARGET_BLOCK_BYTES, 2_621_440);
}

#[test]
fn validation_accepts_every_boundary_value_and_rejects_one_past() {
    let at = |f: fn(&mut FeeConfig)| {
        let mut c = FeeConfig::TESTING;
        f(&mut c);
        c.validate()
    };
    assert_eq!(at(|c| c.treasury_bps = MAX_TREASURY_BPS), Ok(()));
    assert_eq!(
        at(|c| c.treasury_bps = MAX_TREASURY_BPS + 1),
        Err(ConfigError::TreasuryShare(MAX_TREASURY_BPS + 1))
    );
    assert_eq!(at(|c| c.initial_base_fee = c.min_base_fee), Ok(()));
    assert_eq!(
        at(|c| c.target_block_bytes = MIN_TARGET_BLOCK_BYTES),
        Ok(())
    );
    assert_eq!(
        at(|c| c.target_block_bytes = MIN_TARGET_BLOCK_BYTES - 1),
        Err(ConfigError::Target(MIN_TARGET_BLOCK_BYTES - 1))
    );
    assert_eq!(
        at(|c| c.target_block_bytes = MAX_TARGET_BLOCK_BYTES),
        Ok(())
    );
    assert_eq!(
        at(|c| c.target_block_bytes = MAX_TARGET_BLOCK_BYTES + 1),
        Err(ConfigError::Target(MAX_TARGET_BLOCK_BYTES + 1))
    );
}

#[test]
fn a_fee_of_exactly_the_base_fee_is_accepted_and_one_less_is_not() {
    let c = FeeConfig::TESTING;
    let claim = [FeeClaim {
        beneficiary: [7; 32],
    }];
    let size = 1_000;
    let exact = TxFee {
        size_bytes: size,
        max_fee: c
            .initial_base_fee
            .checked_mul(maya_amount::Amount::from_u64(size))
            .unwrap(),
        max_tip: maya_amount::Amount::ZERO,
    };
    assert!(apply_block_fees(&c, 1, None, &[exact], &claim).is_ok());
    let short = TxFee {
        max_fee: exact.max_fee.checked_sub(maya_amount::Amount::ONE).unwrap(),
        ..exact
    };
    assert!(apply_block_fees(&c, 1, None, &[short], &claim).is_err());
}
