//! The fee market's parameters, and the switch nobody has thrown.
//!
//! Shaped like `crypto/dag/registry.rs`'s `DagConfig`: a struct with an
//! `activation_height`, a [`FeeConfig::DISABLED`] whose activation is
//! `u64::MAX`, and a [`FeeConfig::TESTING`] that tests use. Nothing in
//! `consensus/chain.rs` or `state/db.rs` reads any of it — the same discipline
//! `lattice-pow` and `blockgraph` are held to, and `tests/fee_market_tests.rs`
//! checks it.
//!
//! # Not governance parameters, yet
//!
//! These do not join `governance::ParameterKey`. That table is enumerated at
//! genesis, so adding keys changes the genesis state root of every existing
//! chain. When the fee market activates, the governable subset moves into it —
//! and from then on they are read from state, never from these constants
//! (invariant 17).

use crate::limits::{
    MAX_CHANGE_DENOMINATOR, MAX_TARGET_BLOCK_BYTES, MAX_TREASURY_BPS, MIN_BASE_FEE_FLOOR,
    MIN_CHANGE_DENOMINATOR, MIN_TARGET_BLOCK_BYTES,
};

/// Fee market parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeeConfig {
    /// First height at which fees are charged under these rules.
    pub activation_height: u64,
    /// The block size, in serialized bytes, the base fee steers towards.
    pub target_block_bytes: u64,
    /// The base fee's lowest value, per byte.
    pub min_base_fee: u64,
    /// The base fee moves by at most `1 / change_denominator` per block.
    pub change_denominator: u64,
    /// The treasury's share of each base fee, in basis points; the rest burns.
    pub treasury_bps: u64,
    /// The base fee at the activation height, before any adjustment.
    pub initial_base_fee: u64,
    /// First height at which the base fee follows [`crate::FeeRule::Neural`]
    /// with [`crate::MODEL_V1`] instead of the linear step. A second switch,
    /// `u64::MAX` in every shipped configuration — see `docs/neural-gas.md`.
    pub neural_activation_height: u64,
}

/// A configuration outside [`crate::limits`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// The change denominator is outside its bounds.
    ChangeRate(u64),
    /// The base fee floor is below the minimum.
    Floor(u64),
    /// The block-size target is outside its bounds.
    Target(u64),
    /// The treasury share is above its ceiling.
    TreasuryShare(u64),
    /// The initial base fee is below the floor.
    InitialBelowFloor,
}

impl FeeConfig {
    /// The configuration every network runs: charged nowhere.
    pub const DISABLED: Self = Self {
        activation_height: u64::MAX,
        ..Self::TESTING
    };

    /// Active from height 1, with the parameters the brief asked for: an
    /// 80/20 burn/treasury split and EIP-1559's one-eighth step.
    pub const TESTING: Self = Self {
        activation_height: 1,
        target_block_bytes: 1024 * 1024,
        min_base_fee: 1,
        change_denominator: 8,
        treasury_bps: 2_000,
        initial_base_fee: 10,
        neural_activation_height: u64::MAX,
    };

    /// [`FeeConfig::TESTING`] with the neural gain active from the start.
    pub const TESTING_NEURAL: Self = Self {
        neural_activation_height: 1,
        ..Self::TESTING
    };

    /// Whether fees are charged at `height`.
    #[must_use]
    pub const fn is_active(&self, height: u64) -> bool {
        height >= self.activation_height
    }

    /// Whether the base fee follows the neural gain at `height`. Requires the
    /// fee market itself to be active.
    #[must_use]
    pub const fn is_neural_active(&self, height: u64) -> bool {
        self.is_active(height) && height >= self.neural_activation_height
    }

    /// Checks every parameter against the compiled-in bounds.
    ///
    /// # Errors
    ///
    /// The first [`ConfigError`] found.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(MIN_CHANGE_DENOMINATOR..=MAX_CHANGE_DENOMINATOR).contains(&self.change_denominator) {
            return Err(ConfigError::ChangeRate(self.change_denominator));
        }
        if self.min_base_fee < MIN_BASE_FEE_FLOOR {
            return Err(ConfigError::Floor(self.min_base_fee));
        }
        if !(MIN_TARGET_BLOCK_BYTES..=MAX_TARGET_BLOCK_BYTES).contains(&self.target_block_bytes) {
            return Err(ConfigError::Target(self.target_block_bytes));
        }
        if self.treasury_bps > MAX_TREASURY_BPS {
            return Err(ConfigError::TreasuryShare(self.treasury_bps));
        }
        if self.initial_base_fee < self.min_base_fee {
            return Err(ConfigError::InitialBelowFloor);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_disabled_configuration_is_active_nowhere() {
        assert_eq!(FeeConfig::DISABLED.activation_height, u64::MAX);
        assert!(!FeeConfig::DISABLED.is_active(u64::MAX - 1));
    }

    #[test]
    fn both_shipped_configurations_are_within_the_limits() {
        FeeConfig::TESTING.validate().expect("testing");
        FeeConfig::DISABLED.validate().expect("disabled");
    }

    #[test]
    fn every_parameter_is_bounded() {
        let base = FeeConfig::TESTING;
        let cases = [
            (
                FeeConfig {
                    change_denominator: 2,
                    ..base
                },
                ConfigError::ChangeRate(2),
            ),
            (
                FeeConfig {
                    change_denominator: 4096,
                    ..base
                },
                ConfigError::ChangeRate(4096),
            ),
            (
                FeeConfig {
                    min_base_fee: 0,
                    initial_base_fee: 0,
                    ..base
                },
                ConfigError::Floor(0),
            ),
            (
                FeeConfig {
                    target_block_bytes: 1024,
                    ..base
                },
                ConfigError::Target(1024),
            ),
            (
                FeeConfig {
                    treasury_bps: 10_000,
                    ..base
                },
                ConfigError::TreasuryShare(10_000),
            ),
            (
                FeeConfig {
                    initial_base_fee: 0,
                    ..base
                },
                ConfigError::InitialBelowFloor,
            ),
        ];
        for (config, expected) in cases {
            assert_eq!(config.validate(), Err(expected));
        }
    }
}
