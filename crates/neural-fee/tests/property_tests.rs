//! Property-based tests for the neural rule and model, moved from
//! `maya-fee-market/tests/property_tests.rs` on 2026-11-14.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_amount::Amount;
use maya_fee_market::next_base_fee;
use maya_neural_fee::model::{MAX_GAIN_BPS, UNIT_GAIN_BPS};
use maya_neural_fee::{Features, MODEL_V1, neural_next_base_fee};
use proptest::prelude::*;

proptest! {
    /// The neural rule's step should never exceed 2x the linear step (gain capped at 2x).
    #[test]
    fn neural_step_at_most_2x_linear(
        base_fee in 1u64..1_000_000u64,
        target in 1u64..10_000_000u64,
        parent_size in 0u64..10_000_000u64,
        change_denom in 8u64..1000u64,  // ADR-076: next_base_fee debug-asserts denominator >= MIN_CHANGE_DENOMINATOR
    ) {
        // neural_next_base_fee(parent_base_fee, parent_size, target, denominator, floor, gain_bps)
        let fee = neural_next_base_fee(Amount::from_u64(base_fee), parent_size, target, change_denom, Amount::from_u64(1), UNIT_GAIN_BPS);
        let linear_fee = next_base_fee(Amount::from_u64(base_fee), parent_size, target, change_denom, Amount::from_u64(1));

        // The neural fee should be between linear and 2x linear (for rising)
        // For falling, it should be between 0 and linear
        if parent_size > target {
            // Rising: fee should be >= linear and <= 2*linear
            prop_assert!(fee >= linear_fee, "fee {} < linear {}", fee, linear_fee);
            prop_assert!(fee <= linear_fee.saturating_add(linear_fee), "fee {} > 2*linear {}", fee, linear_fee);
        } else if parent_size < target {
            // Falling: fee should be >= floor and <= linear
            prop_assert!(fee <= linear_fee, "fee {} > linear {}", fee, linear_fee);
            prop_assert!(fee >= Amount::from_u64(1), "fee {} < floor 1", fee);
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
}
