//! Labelled blocks.
//!
//! For each simulated block the label is the gain that would have set the next
//! block's demand to exactly the target. With unit elasticity that fee is
//! `f · D_next(f) / target`, so the desired step is known in closed form and
//! the gain is that step over the linear rule's.
//!
//! Half the trajectory is driven by the linear rule and half by random gains,
//! so the network sees the states its own choices would lead to rather than
//! only the states the linear rule visits.

use maya_fee_market::{Features, neural_next_base_fee};

use crate::rng::SplitMix64;
use crate::simulator::{Market, TARGET_BYTES, features};

/// EIP-1559's denominator, as in `FeeConfig::TESTING`.
pub const DENOMINATOR: u64 = 8;

/// The fee floor.
pub const FLOOR: u64 = 1;

/// The fee a simulation starts at.
pub const INITIAL_FEE: u64 = 1_000;

/// The largest label: twice the linear step.
pub const MAX_GAIN: f64 = 2.0;

/// One labelled block.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    /// What the network sees.
    pub features: Features,
    /// The gain that would have been right, in `[0, MAX_GAIN]`.
    pub gain: f64,
}

/// Simulates `blocks` blocks and labels every one that is off target.
#[must_use]
pub fn generate(seed: u64, blocks: usize) -> Vec<Sample> {
    let mut market = Market::new(seed);
    let mut behaviour = SplitMix64::new(seed.rotate_left(17));
    let mut fee = INITIAL_FEE;
    let mut previous_size = TARGET_BYTES;
    let mut samples = Vec::with_capacity(blocks);

    for index in 0..blocks {
        market.advance();
        let block = market.block(fee);
        let block_features = features(&block, previous_size);

        if let Some(gain) = label(&market, fee, block.size) {
            samples.push(Sample {
                features: block_features,
                gain,
            });
        }

        let behaviour_gain = if index % 2 == 0 {
            10_000
        } else {
            (behaviour.unit() * 20_000.0) as u64
        };
        fee = neural_next_base_fee(
            fee,
            block.size,
            TARGET_BYTES,
            DENOMINATOR,
            FLOOR,
            behaviour_gain,
        );
        previous_size = block.size;
    }
    samples
}

/// The ideal gain for a block of `size` at `fee`, or `None` for a block at
/// target, where every gain does the same thing.
fn label(market: &Market, fee: u64, size: u64) -> Option<f64> {
    if size == TARGET_BYTES {
        return None;
    }
    let mut next = market.clone();
    next.advance();
    let fee_f = fee as f64;
    let target = TARGET_BYTES as f64;
    let ideal = fee_f * next.demand_at(fee) / target;
    let desired = ideal - fee_f;

    let gap = size as f64 - target;
    let linear = fee_f * gap / target / DENOMINATOR as f64;
    if linear == 0.0 {
        return None;
    }
    // The rule clamps a rise's gain to [1, 2] and a fall's to [0, 1], so the
    // label does the same: outside that range every gain does the same thing.
    let ratio = desired / linear;
    Some(if gap > 0.0 {
        ratio.clamp(1.0, MAX_GAIN)
    } else {
        ratio.clamp(0.0, 1.0)
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn labels_are_in_range_and_reproducible() {
        let a = generate(11, 2_000);
        let b = generate(11, 2_000);
        assert!(!a.is_empty());
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert!((0.0..=MAX_GAIN).contains(&x.gain));
            assert_eq!(x.gain.to_bits(), y.gain.to_bits());
            assert_eq!(x.features, y.features);
        }
    }
}
