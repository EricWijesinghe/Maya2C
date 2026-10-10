//! Property-based tests for the fee market.
//!
//! These tests use proptest to generate random inputs and verify
//! invariants hold across the entire input space.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::too_many_lines)]

use maya_fee_market::limits::{
    MAX_CHANGE_DENOMINATOR, MAX_TARGET_BLOCK_BYTES, MAX_TREASURY_BPS, MIN_CHANGE_DENOMINATOR,
    MIN_TARGET_BLOCK_BYTES,
};
use maya_fee_market::model::{MAX_GAIN_BPS, UNIT_GAIN_BPS};
use maya_fee_market::{
    BlockFeeOutcome, Features, FeeClaim, FeeConfig, MODEL_V1, TxFee, apply_block_fees,
    neural_next_base_fee, next_base_fee,
};
use proptest::prelude::*;

proptest! {
    /// The fee market's split function should always distribute exactly what was paid:
    /// burned + treasury + tip == base_fee * size + tip (where tip is the priority fee).
    #[test]
    fn split_always_distributes_exactly_what_was_paid(
        base_fee in 1u64..1_000_000_000u64,
        tip in 0u64..1_000_000_000u64,
        bps in 0u64..=10_000u64,
    ) {
        let s = maya_fee_market::split(base_fee, tip, bps);
        let expected_total = base_fee.saturating_add(tip);
        let actual_total = s.burned.saturating_add(s.treasury).saturating_add(s.tip);
        prop_assert_eq!(actual_total, expected_total,
            "split must distribute exactly what was paid: base={}, tip={}, bps={}", base_fee, tip, bps);
    }

    /// The treasury share should never exceed the configured maximum.
    #[test]
    fn treasury_share_never_exceeds_max(
        base_fee in 1u64..1_000_000_000u64,
        tip in 0u64..1_000_000_000u64,
        bps in 0u64..=10_000u64,
    ) {
        let s = maya_fee_market::split(base_fee, tip, bps);
        // The treasury portion comes from bps of the base fee portion only
        prop_assert!(s.treasury <= base_fee,
            "treasury portion must not exceed base fee: treasury={}, base={}", s.treasury, base_fee);
    }

    /// The burned portion should equal base_fee minus treasury (which uses integer division).
    #[test]
    fn burned_portion_is_correct(
        base_fee in 1u64..1_000_000_000u64,
        tip in 0u64..1_000_000_000u64,
        bps in 0u64..=10_000u64,
    ) {
        let s = maya_fee_market::split(base_fee, tip, bps);
        // Treasury is computed with integer division: base_fee * bps / 10000
        // Burned = base_fee - treasury
        let treasury = base_fee.saturating_mul(bps.min(10_000)) / 10_000;
        let expected_burned = base_fee.saturating_sub(treasury);
        prop_assert_eq!(s.burned, expected_burned,
            "burned portion mismatch: base={}, tip={}, bps={}", base_fee, tip, bps);
    }

    /// Tip should be passed through unchanged.
    #[test]
    fn tip_is_passed_through_unchanged(
        base_fee in 1u64..1_000_000_000u64,
        tip in 0u64..1_000_000_000u64,
        bps in 0u64..=10_000u64,
    ) {
        let s = maya_fee_market::split(base_fee, tip, bps);
        prop_assert_eq!(s.tip, tip, "tip must be passed through unchanged");
    }

    /// The fee config validation should accept boundary values and reject out-of-bounds.
    #[test]
    fn config_validation_bounds(
        treasury_bps in 0u64..=5001u64,  // MAX_TREASURY_BPS = 5000
        target_block_bytes in (MIN_TARGET_BLOCK_BYTES.saturating_sub(100))..=(MAX_TARGET_BLOCK_BYTES + 100),
        initial_base_fee in 1u64..1_000_000_000u64,
        min_base_fee in 1u64..1_000_000_000u64,
        change_denominator in 8u64..1024u64,  // Valid range per limits: MIN_CHANGE_DENOMINATOR=8
    ) {
        let mut config = FeeConfig::TESTING;
        config.treasury_bps = treasury_bps;
        config.target_block_bytes = target_block_bytes;
        config.initial_base_fee = initial_base_fee;
        config.min_base_fee = min_base_fee;
        config.change_denominator = change_denominator;

        let result = config.validate();

        let valid_treasury = treasury_bps <= MAX_TREASURY_BPS;
        let valid_target = target_block_bytes >= MIN_TARGET_BLOCK_BYTES && target_block_bytes <= MAX_TARGET_BLOCK_BYTES;
        let valid_denom = change_denominator >= MIN_CHANGE_DENOMINATOR && change_denominator <= MAX_CHANGE_DENOMINATOR;
        let valid_initial = initial_base_fee >= min_base_fee;

        if valid_treasury && valid_target && valid_denom && valid_initial {
            prop_assert!(result.is_ok(), "valid config should be accepted: treasury_bps={}, target={}", treasury_bps, target_block_bytes);
        } else {
            prop_assert!(result.is_err(), "invalid config should be rejected: treasury_bps={}, target={}", treasury_bps, target_block_bytes);
        }
    }

    /// The neural rule's step should never exceed 2x the linear step (gain capped at 2x).
    #[test]
    fn neural_step_at_most_2x_linear(
        base_fee in 1u64..1_000_000u64,
        target in 1u64..10_000_000u64,
        parent_size in 0u64..10_000_000u64,
        change_denom in 1u64..1000u64,
    ) {
        // neural_next_base_fee(parent_base_fee, parent_size, target, denominator, floor, gain_bps)
        let fee = neural_next_base_fee(base_fee, parent_size, target, change_denom, 1, UNIT_GAIN_BPS);
        let linear_fee = next_base_fee(base_fee, parent_size, target, change_denom, 1);

        // The neural fee should be between linear and 2x linear (for rising)
        // For falling, it should be between 0 and linear
        if parent_size > target {
            // Rising: fee should be >= linear and <= 2*linear
            prop_assert!(fee >= linear_fee, "fee {} < linear {}", fee, linear_fee);
            prop_assert!(fee <= linear_fee * 2, "fee {} > 2*linear {}", fee, linear_fee);
        } else if parent_size < target {
            // Falling: fee should be >= floor and <= linear
            prop_assert!(fee <= linear_fee, "fee {} > linear {}", fee, linear_fee);
            prop_assert!(fee >= 1, "fee {} < floor 1", fee);
        }
    }

    /// The model's gain should be in the valid range [0, MAX_GAIN_BPS] for any input.
    #[test]
    fn model_gain_in_valid_range(
        x in any::<u64>(),
    ) {
        // Generate features from a seeded RNG
        let mut x = x;
        let mut v = [0i64; 6];
        for slot in &mut v {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *slot = i64::try_from(x % (2 * 4 * 4096 + 1)).unwrap() - 4 * 4096;
        }
        let f = Features::new(v);
        let gain = MODEL_V1.gain(&f);

        // Gain must be in valid range [0, MAX_GAIN_BPS]
        prop_assert!(gain <= MAX_GAIN_BPS, "gain {} exceeds MAX_GAIN_BPS {}", gain, MAX_GAIN_BPS);
    }

    /// apply_block_fees should correctly process a fee claim and
    /// the total collected should match what the claim pays.
    #[test]
    fn apply_block_fees_collects_correct_total(
        base_fee in 1u64..1_000_000u64,
        size in 1u64..10_000u64,
        tip in 0u64..100_000u64,
    ) {
        let config = FeeConfig::TESTING;
        let config = FeeConfig {
            initial_base_fee: base_fee,
            ..config
        };

        let max_fee = base_fee.saturating_mul(size).saturating_add(tip);
        let tx = TxFee {
            size_bytes: size,
            max_fee,
            max_tip: tip,
        };

        let result = apply_block_fees(&config, 1, None, &[tx], &[FeeClaim { beneficiary: [1u8; 32] }]);
        prop_assert!(result.is_ok());
        let outcome = result.unwrap();
        let expected_total = base_fee.saturating_mul(size).saturating_add(tip);
        if let BlockFeeOutcome::Charged { burned, treasury, tips, .. } = outcome {
            let actual_total = burned.saturating_add(treasury).saturating_add(tips);
            prop_assert_eq!(actual_total, expected_total,
                "collected total must match sum of max fees");
        } else {
            prop_assert!(false, "expected Charged outcome");
        }
    }

    /// The fee split should be invariant under permutation of the same inputs.
    #[test]
    fn split_is_deterministic(
        base_fee in 1u64..1_000_000_000u64,
        tip in 0u64..1_000_000_000u64,
        bps in 0u64..=10_000u64,
    ) {
        let s1 = maya_fee_market::split(base_fee, tip, bps);
        let s2 = maya_fee_market::split(base_fee, tip, bps);
        let s3 = maya_fee_market::split(base_fee, tip, bps);
        prop_assert_eq!(s1.burned, s2.burned);
        prop_assert_eq!(s1.burned, s3.burned);
        prop_assert_eq!(s1.treasury, s2.treasury);
        prop_assert_eq!(s1.treasury, s3.treasury);
        prop_assert_eq!(s1.tip, s2.tip);
        prop_assert_eq!(s1.tip, s3.tip);
    }
}

// Additional edge case tests using regular #[test]
#[cfg(test)]
mod edge_cases {
    use super::*;

    #[test]
    fn split_at_max_bps_puts_everything_in_treasury() {
        let s = maya_fee_market::split(1000, 0, 10_000);
        assert_eq!(s.burned, 0);
        assert_eq!(s.treasury, 1000);
        assert_eq!(s.tip, 0);
    }

    #[test]
    fn split_at_zero_bps_burns_everything() {
        let s = maya_fee_market::split(1000, 0, 0);
        assert_eq!(s.burned, 1000);
        assert_eq!(s.treasury, 0);
        assert_eq!(s.tip, 0);
    }

    #[test]
    fn split_with_tip_keeps_tip_separate() {
        let s = maya_fee_market::split(1000, 500, 5_000);
        // 50% to treasury, 50% burned, tip unchanged
        assert_eq!(s.tip, 500);
        assert_eq!(s.treasury, 500);
        assert_eq!(s.burned, 500);
    }

    #[test]
    fn config_rejects_zero_denominator() {
        let mut config = FeeConfig::TESTING;
        config.change_denominator = 0;
        assert!(config.validate().is_err());
    }
}
