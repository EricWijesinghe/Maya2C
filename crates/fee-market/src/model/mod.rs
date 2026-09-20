//! An integer neural network that sets the gain on EIP-1559's step.
//!
//! # What it decides, and what it cannot
//!
//! [`Model::gain`] maps six block features to a gain in basis points, from 0
//! to [`MAX_GAIN_BPS`]: how much of the linear rule's step to take. 10,000 is
//! the linear rule exactly. The gain is all the model controls.
//! [`crate::rule::neural_next_base_fee`] then clamps it to `[1, 2]` for a rise
//! and `[0, 1]` for a fall, so no weight file, however trained, can price a
//! block below EIP-1559 — the direction a producer who writes the features
//! would want. The model can only make rises faster and falls slower.
//!
//! # Integers only
//!
//! A float in a consensus rule is a rounding mode two validators can disagree
//! on (invariant 20). Features are Q16, weights are `i16` in Q12, biases `i32`
//! in Q28, and every product lands in an `i64` accumulator. The bound is by
//! construction, not by testing:
//!
//! ```text
//! |feature| ≤ 4·2^16 = 2^18       (Features::new clamps)
//! |weight|  ≤ 2^15                (i16)
//! hidden:  6 products < 6·2^33, plus a bias < 2^31  →  < 2^36; >> 12 → < 2^24
//! output: 16 products < 16·2^15·2^24 = 2^43, plus a bias  →  < 2^44
//! gain:   (output >> 12) < 2^32;  · 10,000 < 2^46;  >> 16 < 2^30   — inside i64
//! ```
//!
//! The order is shift, multiply, shift — so the widest intermediate is 2^46,
//! and even multiplying before the first shift would stay below 2^58.
//!
//! # No zero-knowledge proof
//!
//! Every validator has the features and the weights, so every validator runs
//! the 112 multiply-accumulates itself. A proof of inference would prove only
//! what re-running proves, at roughly ten thousand times the cost
//! (`docs/zkml.md`, "The economics").
//!
//! # No governance key
//!
//! The weights are a compiled-in `const`, selected by activation height as
//! `crypto/dag/registry.rs` selects a hash. A network whose weights were a
//! governable value would be a network whose fee rule is a program somebody
//! uploads (invariant 13).

pub mod weights_v1;

/// Inputs the network reads.
pub const INPUTS: usize = 6;

/// Hidden units.
pub const HIDDEN: usize = 16;

/// Fractional bits of a feature.
pub const FEATURE_FRAC_BITS: u32 = 16;

/// A feature value of 1.0.
pub const FEATURE_ONE: i64 = 1 << FEATURE_FRAC_BITS;

/// The largest magnitude a feature may carry: 4.0.
pub const FEATURE_LIMIT: i64 = 4 * FEATURE_ONE;

/// Fractional bits of a weight. A bias carries `FEATURE_FRAC_BITS` more.
pub const WEIGHT_FRAC_BITS: u32 = 12;

/// The linear rule's gain, in basis points.
pub const UNIT_GAIN_BPS: u64 = 10_000;

/// The largest gain the network may return: twice the linear step.
pub const MAX_GAIN_BPS: u64 = 2 * UNIT_GAIN_BPS;

/// Multiply-accumulates per inference: the whole cost of [`Model::gain`].
pub const MULTIPLY_ACCUMULATES: usize = INPUTS * HIDDEN + HIDDEN;

/// Which feature sits at which index. The node's extractor fills them in
/// this order; `tests/neural_fee_tests.rs` pins the two sides together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// Block bytes over target.
    Fullness = 0,
    /// Mean transaction bytes over 16 KiB.
    MeanTxSize = 1,
    /// Share of transactions touching an account another transaction touches.
    AccessOverlap = 2,
    /// Declared contract fuel per byte, over 1,000.
    FuelPerByte = 3,
    /// Share of transactions whose accounts span more than one shard.
    CrossShard = 4,
    /// Change in block bytes from the block before, over target. Signed.
    SizeTrend = 5,
}

/// Six clamped Q16 features.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Features([i64; INPUTS]);

impl Features {
    /// Every feature zero.
    pub const ZERO: Self = Self([0; INPUTS]);

    /// Clamps every value into `±FEATURE_LIMIT`, which is what makes the
    /// accumulator bound above hold for any caller.
    #[must_use]
    pub fn new(values: [i64; INPUTS]) -> Self {
        Self(values.map(|value| value.clamp(-FEATURE_LIMIT, FEATURE_LIMIT)))
    }

    /// The clamped values.
    #[must_use]
    pub const fn values(&self) -> &[i64; INPUTS] {
        &self.0
    }
}

/// A 6-16-1 network with integer ReLU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Model {
    /// Hidden weights, Q12, one row per hidden unit.
    pub hidden_weights: [[i16; INPUTS]; HIDDEN],
    /// Hidden biases, Q28.
    pub hidden_bias: [i32; HIDDEN],
    /// Output weights, Q12.
    pub output_weights: [i16; HIDDEN],
    /// Output bias, Q28. The gain before any feature speaks, in Q16 after the
    /// shift: `1 << 28` is a gain of 1.0.
    pub output_bias: i32,
}

impl Model {
    /// A network whose every weight is zero: gain 0, so the fee never moves.
    pub const ZERO: Self = Self {
        hidden_weights: [[0; INPUTS]; HIDDEN],
        hidden_bias: [0; HIDDEN],
        output_weights: [0; HIDDEN],
        output_bias: 0,
    };

    /// The gain for one block, in basis points, in `[0, MAX_GAIN_BPS]`.
    #[must_use]
    pub fn gain(&self, features: &Features) -> u64 {
        let mut output = i64::from(self.output_bias);
        for ((row, bias), weight) in self
            .hidden_weights
            .iter()
            .zip(self.hidden_bias)
            .zip(self.output_weights)
        {
            let hidden = hidden_unit(row, bias, features);
            output += i64::from(weight) * hidden;
        }
        // Q28 → Q16, then Q16 → basis points.
        let gain = ((output >> WEIGHT_FRAC_BITS) * UNIT_GAIN_BPS as i64) >> FEATURE_FRAC_BITS;
        // Clamped into [0, MAX_GAIN_BPS], which fits u64 exactly.
        gain.clamp(0, MAX_GAIN_BPS as i64) as u64
    }
}

/// One hidden unit: `relu((w·x + b) >> 12)`, in Q16.
fn hidden_unit(row: &[i16; INPUTS], bias: i32, features: &Features) -> i64 {
    let mut sum = i64::from(bias);
    for (&weight, &value) in row.iter().zip(features.values()) {
        sum += i64::from(weight) * value;
    }
    (sum >> WEIGHT_FRAC_BITS).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every weight and bias at its most negative or most positive.
    fn extreme(weight: i16, bias: i32) -> Model {
        Model {
            hidden_weights: [[weight; INPUTS]; HIDDEN],
            hidden_bias: [bias; HIDDEN],
            output_weights: [weight; HIDDEN],
            output_bias: bias,
        }
    }

    #[test]
    fn the_accumulator_bound_holds_at_every_extreme() {
        // Debug builds panic on overflow, so reaching the assertions is the
        // proof that none of these overflowed.
        for model in [
            extreme(i16::MAX, i32::MAX),
            extreme(i16::MIN, i32::MIN),
            extreme(i16::MIN, i32::MAX),
            extreme(i16::MAX, i32::MIN),
        ] {
            for value in [FEATURE_LIMIT, -FEATURE_LIMIT, 0] {
                let gain = model.gain(&Features::new([value; INPUTS]));
                assert!(gain <= MAX_GAIN_BPS);
            }
        }
    }

    #[test]
    fn features_are_clamped_on_construction() {
        let features = Features::new([i64::MAX, i64::MIN, 5, -5, FEATURE_LIMIT + 1, 0]);
        assert_eq!(
            features.values(),
            &[FEATURE_LIMIT, -FEATURE_LIMIT, 5, -5, FEATURE_LIMIT, 0]
        );
    }

    #[test]
    fn an_output_bias_of_one_is_the_linear_gain() {
        let model = Model {
            output_bias: 1 << (FEATURE_FRAC_BITS + WEIGHT_FRAC_BITS),
            ..Model::ZERO
        };
        assert_eq!(model.gain(&Features::new([0; INPUTS])), UNIT_GAIN_BPS);
        assert_eq!(Model::ZERO.gain(&Features::new([FEATURE_ONE; INPUTS])), 0);
    }

    #[test]
    fn the_gain_is_clamped_to_its_range() {
        let high = Model {
            output_bias: i32::MAX,
            ..Model::ZERO
        };
        assert_eq!(high.gain(&Features::new([0; INPUTS])), MAX_GAIN_BPS);
        let low = Model {
            output_bias: i32::MIN,
            ..Model::ZERO
        };
        assert_eq!(low.gain(&Features::new([0; INPUTS])), 0);
    }

    #[test]
    fn the_cost_is_112_multiply_accumulates() {
        assert_eq!(MULTIPLY_ACCUMULATES, 112);
    }
}
