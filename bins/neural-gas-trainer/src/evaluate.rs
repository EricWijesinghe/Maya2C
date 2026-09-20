//! Running the market closed-loop under a rule, and checking the envelope on
//! every block.

use maya_fee_market::model::UNIT_GAIN_BPS;
use maya_fee_market::{Model, neural_next_base_fee, next_base_fee};

use crate::dataset::{DENOMINATOR, FLOOR, INITIAL_FEE};
use crate::simulator::{MAX_BLOCK_BYTES, Market, TARGET_BYTES, features};

/// Which rule sets the fee.
#[derive(Clone, Copy, Debug)]
pub enum Controller<'a> {
    /// EIP-1559's linear step.
    Linear,
    /// A constant gain, in basis points.
    ConstantGain(u64),
    /// A network's gain.
    Neural(&'a Model),
}

/// What a closed-loop run did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    /// Blocks run.
    pub blocks: usize,
    /// Mean of `|size − target| / target`.
    pub mean_size_deviation: f64,
    /// Mean of `|Δfee| / fee`.
    pub mean_fee_change: f64,
    /// Share of blocks at the size cap.
    pub saturated_share: f64,
    /// Blocks where the fee moved against fullness, faster than one step, or
    /// below the floor. Zero for every rule this crate ships.
    pub envelope_violations: usize,
}

/// Runs `blocks` blocks from `seed`.
#[must_use]
pub fn run(seed: u64, blocks: usize, controller: Controller<'_>) -> Metrics {
    let mut market = Market::new(seed);
    let mut fee = INITIAL_FEE;
    let mut previous_size = TARGET_BYTES;
    let mut metrics = Metrics::default();
    let (mut deviation, mut change, mut saturated) = (0.0, 0.0, 0usize);

    for _ in 0..blocks {
        market.advance();
        let block = market.block(fee);
        let next = match controller {
            Controller::Linear => next_base_fee(fee, block.size, TARGET_BYTES, DENOMINATOR, FLOOR),
            Controller::ConstantGain(gain) => {
                neural_next_base_fee(fee, block.size, TARGET_BYTES, DENOMINATOR, FLOOR, gain)
            }
            Controller::Neural(model) => {
                let gain = model.gain(&features(&block, previous_size));
                neural_next_base_fee(fee, block.size, TARGET_BYTES, DENOMINATOR, FLOOR, gain)
            }
        };
        if !matches!(controller, Controller::Linear) && !within_envelope(fee, block.size, next) {
            metrics.envelope_violations += 1;
        }

        deviation += block.size.abs_diff(TARGET_BYTES) as f64 / TARGET_BYTES as f64;
        change += next.abs_diff(fee) as f64 / fee as f64;
        saturated += usize::from(block.size >= MAX_BLOCK_BYTES);
        previous_size = block.size;
        fee = next;
    }

    let n = blocks.max(1) as f64;
    metrics.blocks = blocks;
    metrics.mean_size_deviation = deviation / n;
    metrics.mean_fee_change = change / n;
    metrics.saturated_share = saturated as f64 / n;
    metrics
}

/// The neural rule's promises, checked on one block: never below EIP-1559,
/// never against fullness, a rise at most twice EIP-1559's, never below the
/// floor.
#[must_use]
pub fn within_envelope(parent: u64, size: u64, next: u64) -> bool {
    let linear = next_base_fee(parent, size, TARGET_BYTES, DENOMINATOR, FLOOR);
    let direction_and_speed = match size.cmp(&TARGET_BYTES) {
        core::cmp::Ordering::Greater => {
            let linear_rise = linear.saturating_sub(parent);
            next >= parent && next - parent <= linear_rise.saturating_mul(2).max(1)
        }
        core::cmp::Ordering::Less => next <= parent.max(FLOOR),
        core::cmp::Ordering::Equal => next == parent.max(FLOOR),
    };
    next >= linear && next >= FLOOR && direction_and_speed
}

/// The linear rule as a constant gain, for comparisons that must share a code
/// path with the neural rule.
pub const LINEAR_GAIN: Controller<'static> = Controller::ConstantGain(UNIT_GAIN_BPS);
