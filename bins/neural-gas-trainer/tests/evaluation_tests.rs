//! The committed weights, run closed-loop against the simulator.
//!
//! Every figure here is about `src/simulator.rs`. What the tests can say about
//! a real chain is limited to the envelope, which holds for any weights.

use maya_fee_market::model::{FEATURE_LIMIT, INPUTS, MAX_GAIN_BPS, UNIT_GAIN_BPS};
use maya_fee_market::{Features, MODEL_V1, Model, neural_next_base_fee};
use maya_neural_gas_trainer::evaluate::{Controller, LINEAR_GAIN, run, within_envelope};
use maya_neural_gas_trainer::rng::SplitMix64;
use maya_neural_gas_trainer::simulator::TARGET_BYTES;
use maya_neural_gas_trainer::{EVALUATION_BLOCKS, EVALUATION_SEED};

#[test]
fn the_committed_weights_are_a_trained_network() {
    assert_ne!(MODEL_V1, Model::ZERO);
}

#[test]
fn unit_gain_through_the_neural_path_is_the_linear_rule_on_the_simulator() {
    // A gain of exactly 1.0 is inside both the rise clamp and the fall clamp,
    // so the neural path must reproduce the linear rule block for block.
    let linear = run(EVALUATION_SEED, 5_000, Controller::Linear);
    let unit = run(EVALUATION_SEED, 5_000, LINEAR_GAIN);
    assert_eq!(linear.mean_size_deviation, unit.mean_size_deviation);
    assert_eq!(linear.mean_fee_change, unit.mean_fee_change);
}

#[test]
fn no_gain_and_no_weight_file_leaves_the_envelope() {
    for gain in [
        0,
        1,
        UNIT_GAIN_BPS / 2,
        UNIT_GAIN_BPS,
        MAX_GAIN_BPS,
        u64::MAX,
    ] {
        let metrics = run(EVALUATION_SEED, 5_000, Controller::ConstantGain(gain));
        assert_eq!(metrics.envelope_violations, 0, "gain {gain}");
    }
    let committed = run(
        EVALUATION_SEED,
        EVALUATION_BLOCKS,
        Controller::Neural(&MODEL_V1),
    );
    assert_eq!(committed.envelope_violations, 0);
}

#[test]
fn random_features_and_fees_never_leave_the_envelope() {
    let mut rng = SplitMix64::new(0xFEE);
    for _ in 0..20_000 {
        let values = [(); INPUTS]
            .map(|()| (rng.next_u64() % (2 * FEATURE_LIMIT as u64 + 1)) as i64 - FEATURE_LIMIT);
        let parent = rng.next_u64() % 10_000_000 + 1;
        let size = rng.next_u64() % (4 * TARGET_BYTES);
        let gain = MODEL_V1.gain(&Features::new(values));
        let next = neural_next_base_fee(parent, size, TARGET_BYTES, 8, 1, gain);
        assert!(
            within_envelope(parent, size, next),
            "{values:?} {parent} {size} → {next}"
        );
    }
}

#[test]
fn the_committed_model_is_compared_with_the_linear_rule() {
    // Reported, and pinned in the direction training achieved. If a retrain
    // loses to the linear rule this fails, and the right response is to say so
    // in docs/neural-gas.md rather than to delete the test.
    let linear = run(EVALUATION_SEED, EVALUATION_BLOCKS, Controller::Linear);
    let neural = run(
        EVALUATION_SEED,
        EVALUATION_BLOCKS,
        Controller::Neural(&MODEL_V1),
    );
    eprintln!("linear {linear:?}\nneural {neural:?}");
    assert!(
        neural.mean_size_deviation <= linear.mean_size_deviation,
        "neural {} vs linear {}",
        neural.mean_size_deviation,
        linear.mean_size_deviation
    );
}
