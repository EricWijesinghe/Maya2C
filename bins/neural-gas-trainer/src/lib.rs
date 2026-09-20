//! Training the fee market's neural gain, off-chain.
//!
//! 1. [`simulator`] produces blocks from a synthetic demand process whose
//!    assumptions are written in its module docs.
//! 2. [`dataset`] runs it and labels every block with the gain that would have
//!    set the next block's demand to exactly the target.
//! 3. [`network`] trains a float 6-16-1 network on those labels.
//! 4. [`quantize`] turns it into `fee-market`'s integer [`Model`].
//! 5. [`evaluate`] runs the market closed-loop under the linear rule and under
//!    a model, and reports what each did.
//! 6. [`emit`] writes `crates/fee-market/src/model/weights_v1.rs`.
//!
//! Everything is deterministic: one seeded [`rng::SplitMix64`], one thread,
//! a fixed order of float operations.

pub mod dataset;
pub mod emit;
pub mod evaluate;
pub mod network;
pub mod quantize;
pub mod rng;
pub mod simulator;

use maya_fee_market::Model;

/// Seed for the training data and the initial weights.
pub const TRAINING_SEED: u64 = 0x4D41_5941_3243_0001;

/// Seed for evaluation. Different from training, so evaluation is out of
/// sample.
pub const EVALUATION_SEED: u64 = 0x4D41_5941_3243_0002;

/// Blocks simulated for the training set.
pub const TRAINING_BLOCKS: usize = 40_000;

/// Passes over the training set.
pub const EPOCHS: usize = 12;

/// Blocks per closed-loop evaluation.
pub const EVALUATION_BLOCKS: usize = 20_000;

/// What one training run produced.
#[derive(Clone, Debug)]
pub struct TrainingRun {
    /// The integer network.
    pub model: Model,
    /// Mean squared error of the float network's gain on the training set.
    pub float_mse: f64,
    /// Largest disagreement between the float and integer gains on the
    /// training set, in basis points.
    pub max_quantization_error_bps: u64,
    /// Samples trained on.
    pub samples: usize,
}

/// Runs the whole pipeline.
#[must_use]
pub fn train() -> TrainingRun {
    let samples = dataset::generate(TRAINING_SEED, TRAINING_BLOCKS);
    let mut rng = rng::SplitMix64::new(TRAINING_SEED ^ 0xA5A5);
    let mut net = network::Network::new(&mut rng);
    net.train(&samples, EPOCHS, &mut rng);
    let model = quantize::quantize(&net);
    TrainingRun {
        float_mse: net.mse(&samples),
        max_quantization_error_bps: quantize::max_disagreement_bps(&net, &model, &samples),
        samples: samples.len(),
        model,
    }
}
