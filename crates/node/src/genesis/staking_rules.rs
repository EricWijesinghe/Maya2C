//! The staking rules a genesis writes out, and the committee bounds a
//! value-bearing genesis must meet (Eric 2026-11-24).
//!
//! Before this, slashing, jail, unbonding, downtime and commission came from
//! `Params::DEVNET` whatever the file said, and none of them reached the
//! genesis id: two operators could disagree about the penalty for downtime
//! and still agree about block zero. Now a genesis may carry every one of
//! them in `bft.staking.rules`, each required once the section is present,
//! and the section is hashed into the id. A value-bearing chain must carry it.

use serde::{Deserialize, Serialize};

use crate::error::{NodeError, Result};

use super::BftGenesis;

/// Fewest genesis validators a value-bearing chain may start with (C5): seven
/// seats survive two faults (n = 3f + 1 with f = 2), where the four-seat
/// committee floor survives one.
pub const MIN_VALUE_BEARING_VALIDATORS: usize = 7;

/// Slashing, jail, unbonding, downtime and commission. No field has a
/// default: a genesis that names the section names every value.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StakingRules {
    /// Epochs between an unbond and the release of its funds: the window in
    /// which evidence can still reach them.
    pub unbonding_epochs: u64,
    /// Share of everything bonded to a validator burned for a double-sign
    /// (proposal or attestation), basis points.
    pub double_sign_slash_bps: u16,
    /// Share of the self bond burned for downtime, basis points.
    pub downtime_slash_bps: u16,
    /// A validator that authored fewer than this share of its anchor slots in
    /// an epoch is down, basis points.
    pub downtime_threshold_bps: u16,
    /// Epochs a downtime-jailed validator sits out.
    pub jail_epochs: u64,
    /// Highest commission a validator may set, basis points.
    pub max_commission_bps: u16,
}

impl StakingRules {
    /// The values a genesis without the section runs: `Params::DEVNET`'s.
    pub const DEVNET: Self = Self {
        unbonding_epochs: maya_staking::Params::DEVNET.unbonding_epochs,
        double_sign_slash_bps: maya_staking::Params::DEVNET.double_sign_slash_bps,
        downtime_slash_bps: maya_staking::Params::DEVNET.downtime_slash_bps,
        downtime_threshold_bps: maya_staking::Params::DEVNET.downtime_threshold_bps,
        jail_epochs: maya_staking::Params::DEVNET.jail_epochs,
        max_commission_bps: maya_staking::Params::DEVNET.max_commission_bps,
    };

    /// `params` with these rules in place of its own.
    #[must_use]
    pub const fn apply(&self, params: maya_staking::Params) -> maya_staking::Params {
        maya_staking::Params {
            unbonding_epochs: self.unbonding_epochs,
            double_sign_slash_bps: self.double_sign_slash_bps,
            downtime_slash_bps: self.downtime_slash_bps,
            downtime_threshold_bps: self.downtime_threshold_bps,
            jail_epochs: self.jail_epochs,
            max_commission_bps: self.max_commission_bps,
            ..params
        }
    }

    /// Folds the rules into `base`, every value fixed-width, in field order.
    #[must_use]
    pub fn commitment(&self, base: [u8; 32]) -> [u8; 32] {
        let mut h = blake3::Hasher::new_derive_key("maya2c genesis staking rules v1");
        h.update(&base);
        h.update(&self.unbonding_epochs.to_le_bytes());
        h.update(&self.double_sign_slash_bps.to_le_bytes());
        h.update(&self.downtime_slash_bps.to_le_bytes());
        h.update(&self.downtime_threshold_bps.to_le_bytes());
        h.update(&self.jail_epochs.to_le_bytes());
        h.update(&self.max_commission_bps.to_le_bytes());
        *h.finalize().as_bytes()
    }
}

/// Whether `chain_id` names a value-bearing network: any name containing
/// "mainnet", case and spaces ignored, the rule `genesis-ceremony` applies,
/// so `maya2c-mainnet` cannot skip these bounds by spelling.
#[must_use]
pub fn is_value_bearing(chain_id: &str) -> bool {
    chain_id.trim().to_ascii_lowercase().contains("mainnet")
}

/// The committee bounds every DAG-BFT genesis must meet (the certificate
/// log's `MAX_COMMITTEE`, and C4), and the ones a value-bearing genesis must
/// also meet (C5 and the written rules).
///
/// # Errors
///
/// [`NodeError::Decode`] naming the bound that failed.
pub fn check_committee(chain_id: &str, bft: &BftGenesis) -> Result<()> {
    let seats = bft.validators.len();
    if bft.staking.as_ref().is_some_and(|staking| {
        usize::from(staking.max_validators) > crate::consensus::bft::store::MAX_COMMITTEE
    }) {
        return Err(NodeError::Decode(format!(
            "bft.staking.max_validators must be at most {}",
            crate::consensus::bft::store::MAX_COMMITTEE
        )));
    }
    if let Some(staking) = &bft.staking
        && seats > usize::from(staking.max_validators)
    {
        return Err(NodeError::Decode(format!(
            "bft.validators number {seats}, above bft.staking.max_validators {}",
            staking.max_validators
        )));
    }
    if !is_value_bearing(chain_id) {
        return Ok(());
    }
    if seats < MIN_VALUE_BEARING_VALIDATORS {
        return Err(NodeError::Decode(format!(
            "a value-bearing chain starts with at least {MIN_VALUE_BEARING_VALIDATORS} validators; \
             this genesis names {seats}"
        )));
    }
    match &bft.staking {
        Some(staking) if staking.rules.is_some() => Ok(()),
        Some(_) => Err(NodeError::Decode(
            "a value-bearing chain must write bft.staking.rules: no staking rule is inherited \
             from the devnet defaults"
                .to_string(),
        )),
        None => Err(NodeError::Decode(
            "a value-bearing chain must configure bft.staking".to_string(),
        )),
    }
}
