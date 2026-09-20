//! A synthetic fee market. Every number the trainer reports is about this
//! module, not about any real chain.
//!
//! # Assumptions, all inherited by the model
//!
//! - **Demand** is measured in bytes a block would carry at [`REFERENCE_FEE`],
//!   and has unit price elasticity: at fee `f` it is `level · REFERENCE_FEE / f`.
//! - **Calm** demand wanders near 80% of the target with small noise.
//! - **Bursts** start with probability [`BURST_PROBABILITY`] per calm block and
//!   come in three kinds, each with its own duration, demand multiplier and
//!   block composition — so what a block *looks like* carries information about
//!   whether the burst will last. That is the only thing a model can learn here
//!   that the linear rule does not already know.
//! - **Blocks** carry at most twice the target.

use maya_fee_market::model::{FEATURE_ONE, INPUTS};
use maya_fee_market::{Feature, Features};

use crate::rng::SplitMix64;

/// The fee market's target, as in `FeeConfig::TESTING`.
pub const TARGET_BYTES: u64 = 1024 * 1024;

/// The most a block carries.
pub const MAX_BLOCK_BYTES: u64 = 2 * TARGET_BYTES;

/// The fee at which `level` is quoted.
pub const REFERENCE_FEE: f64 = 1_000.0;

/// Calm demand, as a share of the target.
const CALM_SHARE: f64 = 0.8;

/// Chance per calm block that a burst starts.
pub const BURST_PROBABILITY: f64 = 0.03;

/// A kind of demand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regime {
    /// Background traffic.
    Calm,
    /// Many small transfers: short, sharp, low overlap.
    TransferBurst,
    /// Trading against a few pools: long, moderate, high overlap.
    DexBurst,
    /// Contract calls: longest, mild, fuel-heavy.
    ContractBurst,
}

/// What blocks look like in one regime.
#[derive(Clone, Copy, Debug)]
struct Shape {
    blocks: (u32, u32),
    multiplier: (f64, f64),
    mean_tx_bytes: f64,
    overlap: f64,
    fuel_per_byte: f64,
    cross_shard: f64,
}

const fn shape(regime: Regime) -> Shape {
    match regime {
        Regime::Calm => Shape {
            blocks: (0, 0),
            multiplier: (1.0, 1.0),
            mean_tx_bytes: 13_500.0,
            overlap: 0.05,
            fuel_per_byte: 20.0,
            cross_shard: 0.30,
        },
        Regime::TransferBurst => Shape {
            blocks: (2, 6),
            multiplier: (3.0, 5.0),
            mean_tx_bytes: 11_400.0,
            overlap: 0.10,
            fuel_per_byte: 0.0,
            cross_shard: 0.55,
        },
        Regime::DexBurst => Shape {
            blocks: (12, 40),
            multiplier: (1.6, 2.6),
            mean_tx_bytes: 11_800.0,
            overlap: 0.65,
            fuel_per_byte: 0.0,
            cross_shard: 0.20,
        },
        Regime::ContractBurst => Shape {
            blocks: (25, 60),
            multiplier: (1.3, 1.9),
            mean_tx_bytes: 16_000.0,
            overlap: 0.30,
            fuel_per_byte: 900.0,
            cross_shard: 0.40,
        },
    }
}

/// One block's composition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Block {
    /// Bytes carried.
    pub size: u64,
    /// Mean transaction bytes.
    pub mean_tx_bytes: f64,
    /// Share of transactions naming a shared account.
    pub overlap: f64,
    /// Declared fuel per byte.
    pub fuel_per_byte: f64,
    /// Share of transactions spanning shards.
    pub cross_shard: f64,
}

/// The demand process.
#[derive(Clone, Debug)]
pub struct Market {
    rng: SplitMix64,
    calm_level: f64,
    regime: Regime,
    burst_left: u32,
    multiplier: f64,
}

impl Market {
    /// A calm market.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self {
            rng: SplitMix64::new(seed),
            calm_level: CALM_SHARE * TARGET_BYTES as f64,
            regime: Regime::Calm,
            burst_left: 0,
            multiplier: 1.0,
        }
    }

    /// The current regime.
    #[must_use]
    pub const fn regime(&self) -> Regime {
        self.regime
    }

    /// Advances demand by one block.
    pub fn advance(&mut self) {
        let target = TARGET_BYTES as f64;
        let wander = self.rng.range(-0.04, 0.04) * target;
        let pull = 0.1 * (CALM_SHARE * target - self.calm_level);
        self.calm_level = (self.calm_level + wander + pull).max(0.1 * target);

        if self.burst_left > 0 {
            self.burst_left -= 1;
            if self.burst_left == 0 {
                self.regime = Regime::Calm;
                self.multiplier = 1.0;
            }
            return;
        }
        if self.rng.unit() < BURST_PROBABILITY {
            self.regime = match self.rng.int(0, 2) {
                0 => Regime::TransferBurst,
                1 => Regime::DexBurst,
                _ => Regime::ContractBurst,
            };
            let s = shape(self.regime);
            self.burst_left = self.rng.int(s.blocks.0, s.blocks.1);
            self.multiplier = self.rng.range(s.multiplier.0, s.multiplier.1);
        }
    }

    /// Bytes demanded at `fee`.
    #[must_use]
    pub fn demand_at(&self, fee: u64) -> f64 {
        let fee = fee.max(1) as f64;
        self.calm_level * self.multiplier * REFERENCE_FEE / fee
    }

    /// The block this market fills at `fee`.
    pub fn block(&mut self, fee: u64) -> Block {
        let s = shape(self.regime);
        // Calm transactions mix into every burst block.
        let demand = self.demand_at(fee);
        let size = demand.min(MAX_BLOCK_BYTES as f64).max(0.0) as u64;
        let jitter = |rng: &mut SplitMix64, value: f64, spread: f64| {
            (value * rng.range(1.0 - spread, 1.0 + spread)).max(0.0)
        };
        Block {
            size,
            mean_tx_bytes: jitter(&mut self.rng, s.mean_tx_bytes, 0.08),
            overlap: jitter(&mut self.rng, s.overlap, 0.25).min(1.0),
            fuel_per_byte: jitter(&mut self.rng, s.fuel_per_byte, 0.3),
            cross_shard: jitter(&mut self.rng, s.cross_shard, 0.2).min(1.0),
        }
    }
}

/// A block's features, in the node's fixed point and order.
#[must_use]
pub fn features(block: &Block, previous_size: u64) -> Features {
    let one = FEATURE_ONE as f64;
    let q = |value: f64| (value * one).round() as i64;
    let target = TARGET_BYTES as f64;
    let mut values = [0i64; INPUTS];
    values[Feature::Fullness as usize] = q(block.size as f64 / target);
    values[Feature::MeanTxSize as usize] = q(block.mean_tx_bytes / 16_384.0);
    values[Feature::AccessOverlap as usize] = q(block.overlap);
    values[Feature::FuelPerByte as usize] = q(block.fuel_per_byte / 1_000.0);
    values[Feature::CrossShard as usize] = q(block.cross_shard);
    values[Feature::SizeTrend as usize] = q((block.size as f64 - previous_size as f64) / target);
    // `Features::new` clamps to ±FEATURE_LIMIT, as the node's extractor does.
    Features::new(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_market_is_a_function_of_its_seed() {
        let run = |seed| {
            let mut market = Market::new(seed);
            (0..500)
                .map(|_| {
                    market.advance();
                    market.block(1_000).size
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(run(3), run(3));
        assert_ne!(run(3), run(4));
    }

    #[test]
    fn demand_halves_when_the_fee_doubles() {
        let market = Market::new(1);
        let ratio = market.demand_at(1_000) / market.demand_at(2_000);
        assert!((ratio - 2.0).abs() < 1e-9);
    }

    #[test]
    fn every_regime_occurs() {
        let mut market = Market::new(9);
        let mut seen = [false; 4];
        for _ in 0..5_000 {
            market.advance();
            seen[market.regime() as usize] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }
}
