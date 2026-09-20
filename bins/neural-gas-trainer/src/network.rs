//! The float 6-16-1 network, trained by plain SGD.
//!
//! Weights and biases are projected into `±WEIGHT_LIMIT` after every step, so
//! quantization into `i16` Q12 and `i32` Q28 never has to saturate a trained
//! value.

use maya_fee_market::model::{FEATURE_ONE, HIDDEN, INPUTS};

use crate::dataset::{MAX_GAIN, Sample};
use crate::rng::SplitMix64;

/// The largest magnitude a weight or bias may take: just under what `i16` Q12
/// can represent.
pub const WEIGHT_LIMIT: f64 = 7.99;

/// Starting learning rate.
const LEARNING_RATE: f64 = 0.01;

/// A float network with the integer model's shape.
#[derive(Clone, Debug)]
pub struct Network {
    /// Hidden weights.
    pub hidden_weights: [[f64; INPUTS]; HIDDEN],
    /// Hidden biases.
    pub hidden_bias: [f64; HIDDEN],
    /// Output weights.
    pub output_weights: [f64; HIDDEN],
    /// Output bias.
    pub output_bias: f64,
}

/// A feature vector as floats.
#[must_use]
pub fn inputs(sample: &Sample) -> [f64; INPUTS] {
    sample
        .features
        .values()
        .map(|value| value as f64 / FEATURE_ONE as f64)
}

impl Network {
    /// Small random weights, and an output bias of 1.0: before training, the
    /// linear rule.
    #[must_use]
    pub fn new(rng: &mut SplitMix64) -> Self {
        let mut hidden_weights = [[0.0; INPUTS]; HIDDEN];
        for row in &mut hidden_weights {
            for weight in row.iter_mut() {
                *weight = rng.range(-0.5, 0.5);
            }
        }
        let mut output_weights = [0.0; HIDDEN];
        for weight in &mut output_weights {
            *weight = rng.range(-0.1, 0.1);
        }
        Self {
            hidden_weights,
            hidden_bias: [0.1; HIDDEN],
            output_weights,
            output_bias: 1.0,
        }
    }

    /// Hidden activations and the raw output.
    #[must_use]
    pub fn forward(&self, x: &[f64; INPUTS]) -> ([f64; HIDDEN], f64) {
        let mut hidden = [0.0; HIDDEN];
        let mut output = self.output_bias;
        for (unit, (row, bias)) in self.hidden_weights.iter().zip(self.hidden_bias).enumerate() {
            let sum: f64 = row.iter().zip(x).map(|(w, v)| w * v).sum::<f64>() + bias;
            hidden[unit] = sum.max(0.0);
            output += self.output_weights[unit] * hidden[unit];
        }
        (hidden, output)
    }

    /// The gain the float network predicts, clamped like the integer one.
    #[must_use]
    pub fn gain(&self, x: &[f64; INPUTS]) -> f64 {
        self.forward(x).1.clamp(0.0, MAX_GAIN)
    }

    /// Mean squared error of the clamped gain.
    #[must_use]
    pub fn mse(&self, samples: &[Sample]) -> f64 {
        let total: f64 = samples
            .iter()
            .map(|s| (self.gain(&inputs(s)) - s.gain).powi(2))
            .sum();
        total / samples.len().max(1) as f64
    }

    /// SGD over shuffled samples.
    pub fn train(&mut self, samples: &[Sample], epochs: usize, rng: &mut SplitMix64) {
        let mut order: Vec<usize> = (0..samples.len()).collect();
        for epoch in 0..epochs {
            shuffle(&mut order, rng);
            let rate = LEARNING_RATE / (1.0 + epoch as f64);
            for &index in &order {
                self.step(&samples[index], rate);
            }
        }
    }

    fn step(&mut self, sample: &Sample, rate: f64) {
        let x = inputs(sample);
        let (hidden, output) = self.forward(&x);
        let error = (output - sample.gain).clamp(-4.0, 4.0);

        for (unit, &activation) in hidden.iter().enumerate() {
            let back = if activation > 0.0 {
                error * self.output_weights[unit]
            } else {
                0.0
            };
            self.output_weights[unit] =
                project(self.output_weights[unit] - rate * error * activation);
            for (weight, value) in self.hidden_weights[unit].iter_mut().zip(&x) {
                *weight = project(*weight - rate * back * value);
            }
            self.hidden_bias[unit] = project(self.hidden_bias[unit] - rate * back);
        }
        self.output_bias = project(self.output_bias - rate * error);
    }
}

fn project(value: f64) -> f64 {
    value.clamp(-WEIGHT_LIMIT, WEIGHT_LIMIT)
}

/// Fisher-Yates, from the seeded generator.
fn shuffle(order: &mut [usize], rng: &mut SplitMix64) {
    for i in (1..order.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::dataset::generate;

    #[test]
    fn training_lowers_the_error() {
        let samples = generate(21, 3_000);
        let mut rng = SplitMix64::new(5);
        let mut net = Network::new(&mut rng);
        let before = net.mse(&samples);
        net.train(&samples, 3, &mut rng);
        assert!(
            net.mse(&samples) < before,
            "{} → {}",
            before,
            net.mse(&samples)
        );
    }
}
