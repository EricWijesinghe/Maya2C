//! Tests written from `cargo mutants` survivors (reports/08-security.md).
//!
//! Each one pins a boundary a mutant moved without any test noticing:
//! `>` vs `>=` in validation, `*` vs `+` in the limit constants, the exact-fee
//! case of the underpricing check, and the neural model's arithmetic, which
//! had no test that looked at its output at all. Mutants on the equality
//! short-circuit in `neural_next_base_fee` (`parent_size > target` after
//! `parent_size == target` has returned) are equivalent and cannot be killed.

#![allow(clippy::unwrap_used)]

use maya_fee_market::limits::{MAX_TARGET_BLOCK_BYTES, MAX_TREASURY_BPS, MIN_TARGET_BLOCK_BYTES};
use maya_fee_market::model::{
    FEATURE_FRAC_BITS, FEATURE_LIMIT, FEATURE_ONE, INPUTS, MAX_GAIN_BPS, UNIT_GAIN_BPS,
    WEIGHT_FRAC_BITS,
};
use maya_fee_market::{
    ConfigError, Features, FeeClaim, FeeConfig, MODEL_V1, Model, TxFee, apply_block_fees,
    neural_next_base_fee,
};

#[test]
fn limits_are_the_documented_sizes() {
    assert_eq!(MIN_TARGET_BLOCK_BYTES, 65_536);
    assert_eq!(MAX_TARGET_BLOCK_BYTES, 16_777_216);
    assert_eq!(FEATURE_LIMIT, 4 * 65_536);
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
        max_fee: c.initial_base_fee * size,
        max_tip: 0,
    };
    assert!(apply_block_fees(&c, 1, None, &[exact], &claim).is_ok());
    let short = TxFee {
        max_fee: exact.max_fee - 1,
        ..exact
    };
    assert!(apply_block_fees(&c, 1, None, &[short], &claim).is_err());
}

#[test]
fn a_zero_target_or_denominator_means_no_change() {
    assert_eq!(
        neural_next_base_fee(100, 5_000, 0, 8, 1, UNIT_GAIN_BPS),
        100
    );
    assert_eq!(
        neural_next_base_fee(100, 5_000, 1_000, 0, 1, UNIT_GAIN_BPS),
        100
    );
    assert_eq!(
        neural_next_base_fee(0, 5_000, 1_000, 0, 7, UNIT_GAIN_BPS),
        7,
        "the floor still holds"
    );
}

/// The network recomputed from its public weights, in `i128`, without sharing
/// any code with `Model::gain`.
fn reference_gain(m: &Model, f: &Features) -> u64 {
    let mut out = i128::from(m.output_bias);
    for h in 0..m.hidden_bias.len() {
        let mut s = i128::from(m.hidden_bias[h]);
        for i in 0..INPUTS {
            s += i128::from(m.hidden_weights[h][i]) * i128::from(f.values()[i]);
        }
        out += i128::from(m.output_weights[h]) * (s >> WEIGHT_FRAC_BITS).max(0);
    }
    let gain = ((out >> WEIGHT_FRAC_BITS) * i128::from(UNIT_GAIN_BPS)) >> FEATURE_FRAC_BITS;
    u64::try_from(gain.clamp(0, i128::from(MAX_GAIN_BPS))).unwrap()
}

#[test]
fn model_v1_matches_an_independent_evaluation_across_its_input_range() {
    let mut seen = std::collections::BTreeSet::new();
    let mut x = 0x0FEE_u64;
    for _ in 0..2_000 {
        let mut v = [0i64; INPUTS];
        for slot in &mut v {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *slot = i64::try_from(x % (2 * 4 * FEATURE_ONE as u64 + 1)).unwrap() - 4 * FEATURE_ONE;
        }
        let f = Features::new(v);
        let gain = MODEL_V1.gain(&f);
        assert_eq!(gain, reference_gain(&MODEL_V1, &f), "features {v:?}");
        seen.insert(gain);
    }
    // A model whose hidden layer collapsed to a constant would give one value.
    assert!(seen.len() > 50, "only {} distinct gains", seen.len());
    assert_eq!(Model::ZERO.gain(&Features::ZERO), 0);
}

/// FNV-1a over every weight in declaration order. `MODEL_V1` is a consensus
/// parameter: changing one weight changes the fee every node computes, so the
/// table is pinned here and a deliberate retrain updates this digest in the
/// same commit that bumps the model version.
fn weights_digest(m: &Model) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    let mut eat = |v: i64| {
        for b in v.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    m.hidden_weights
        .iter()
        .flatten()
        .for_each(|&w| eat(i64::from(w)));
    m.hidden_bias.iter().for_each(|&b| eat(i64::from(b)));
    m.output_weights.iter().for_each(|&w| eat(i64::from(w)));
    eat(i64::from(m.output_bias));
    h
}

#[test]
fn model_v1_weights_are_pinned() {
    let d = weights_digest(&MODEL_V1);
    println!("MODEL_V1 weights digest {d:#018x}");
    assert_eq!(d, MODEL_V1_DIGEST, "MODEL_V1 changed");
}

/// Measured from the weights at `weights_v1.rs` as committed.
const MODEL_V1_DIGEST: u64 = 0x674f_ab8c_6783_131a;
