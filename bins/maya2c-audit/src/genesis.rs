//! The facts the audit takes from the genesis file: what was allocated, and the
//! emission and epoch parameters. Read from the same JSON the node reads
//! (`crates/node/src/genesis.rs`, `GenesisConfig`); only the fields used here.

use maya_amount::Amount;
use serde::Deserialize;
use serde_json::Value;

use crate::numbers::{MINING_RESERVE, parse_address, parse_amount};

/// What the genesis file says about supply and emission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Allocations, treasury and genesis bonds. The chain's total supply at
    /// height 0, as `GenesisConfig::total_supply` counts it.
    pub genesis_total: Amount,
    /// The allocation to the mining reserve (zero if none).
    pub reserve_genesis: Amount,
    /// Emission parameters, present when the chain runs mining.
    pub mining: Option<MiningParams>,
    /// Blocks per epoch. Present whenever `mining` is.
    pub epoch_blocks: Option<u64>,
}

/// `bft.mining` in the genesis file: the inputs to MINE-1 and MINE-2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MiningParams {
    pub initial_epoch_reward: Amount,
    pub tail_epoch_reward: Amount,
    pub halving_epochs: u64,
    pub miner_share_bps: u64,
}

#[derive(Deserialize)]
struct RawGenesis {
    #[serde(default)]
    allocations: Vec<RawAllocation>,
    treasury: Option<RawAllocation>,
    bft: Option<RawBft>,
}

#[derive(Deserialize)]
struct RawAllocation {
    address: String,
    balance: Value,
}

#[derive(Deserialize)]
struct RawBft {
    staking: Option<RawStaking>,
    mining: Option<RawMining>,
}

#[derive(Deserialize)]
struct RawStaking {
    epoch_blocks: u64,
    #[serde(default)]
    bonds: Vec<RawBond>,
}

#[derive(Deserialize)]
struct RawBond {
    bond: Value,
}

#[derive(Deserialize)]
struct RawMining {
    initial_epoch_reward: Value,
    halving_epochs: u64,
    tail_epoch_reward: Option<Value>,
    miner_share_bps: u64,
}

/// Errors reading the genesis file. Any of them stops the audit: nothing can be
/// checked against a genesis that cannot be read.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GenesisError {
    #[error("genesis is not JSON of the expected shape: {0}")]
    Shape(String),
    #[error("amount `{0}` is not a base-10 U256 (use a decimal string above 2^64)")]
    Amount(String),
    #[error("address `{0}` is not 32 hex bytes")]
    Address(String),
    #[error(
        "bft.mining needs bft.staking.epoch_blocks, a non-zero epoch length and halving_epochs"
    )]
    MiningWithoutEpochs,
    #[error("total supply overflows U256")]
    Overflow,
}

/// Reads a genesis file's JSON text into a [`Plan`].
///
/// # Errors
///
/// See [`GenesisError`].
pub fn parse_plan(text: &str) -> Result<Plan, GenesisError> {
    let raw: RawGenesis =
        serde_json::from_str(text).map_err(|e| GenesisError::Shape(e.to_string()))?;
    let mut total = Amount::ZERO;
    let mut reserve_genesis = Amount::ZERO;
    for allocation in raw.allocations.iter().chain(raw.treasury.iter()) {
        let (address, balance) = allocation_fields(allocation)?;
        total = total.checked_add(balance).ok_or(GenesisError::Overflow)?;
        if address == MINING_RESERVE {
            reserve_genesis = balance;
        }
    }
    let staking = raw.bft.as_ref().and_then(|b| b.staking.as_ref());
    for bond in staking.map_or(&[][..], |s| s.bonds.as_slice()) {
        let amount = amount_value(&bond.bond)?;
        total = total.checked_add(amount).ok_or(GenesisError::Overflow)?;
    }
    // The staking epoch is the fee collector's and the mining epoch's length, so
    // it is read even on a chain without mining (the fee-sink check needs it).
    let epoch_blocks = staking.map(|s| s.epoch_blocks).filter(|e| *e > 0);
    let mining = match raw.bft.as_ref().and_then(|b| b.mining.as_ref()) {
        None => None,
        Some(mining) => {
            if epoch_blocks.is_none() || mining.halving_epochs == 0 {
                return Err(GenesisError::MiningWithoutEpochs);
            }
            let tail = match &mining.tail_epoch_reward {
                Some(value) => amount_value(value)?,
                None => Amount::ZERO,
            };
            Some(MiningParams {
                initial_epoch_reward: amount_value(&mining.initial_epoch_reward)?,
                tail_epoch_reward: tail,
                halving_epochs: mining.halving_epochs,
                miner_share_bps: mining.miner_share_bps,
            })
        }
    };
    Ok(Plan {
        genesis_total: total,
        reserve_genesis,
        mining,
        epoch_blocks,
    })
}

fn allocation_fields(allocation: &RawAllocation) -> Result<([u8; 32], Amount), GenesisError> {
    let address = parse_address(&allocation.address)
        .ok_or_else(|| GenesisError::Address(allocation.address.clone()))?;
    Ok((address, amount_value(&allocation.balance)?))
}

/// The node's lenient amount form: a JSON number (small values, pre-ADR-046
/// files) or a decimal string.
fn amount_value(value: &Value) -> Result<Amount, GenesisError> {
    match value {
        Value::Number(n) => n
            .as_u64()
            .map(Amount::from_u64)
            .ok_or_else(|| GenesisError::Amount(n.to_string())),
        Value::String(s) => parse_amount(s).ok_or_else(|| GenesisError::Amount(s.clone())),
        other => Err(GenesisError::Amount(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn totals_allocations_treasury_and_bonds_and_reads_mining() {
        let text = format!(
            r#"{{
              "allocations": [
                {{"address": "{}", "balance": "1000000"}},
                {{"address": "{}", "balance": 5000}}
              ],
              "treasury": {{"address": "{}", "balance": "7"}},
              "bft": {{
                "staking": {{"epoch_blocks": 4, "bonds": [{{"bond": "300"}}]}},
                "mining": {{
                  "initial_epoch_reward": "1000", "halving_epochs": 2,
                  "tail_epoch_reward": "100", "miner_share_bps": 7000
                }}
              }}
            }}"#,
            hex::encode(MINING_RESERVE),
            "11".repeat(32),
            "22".repeat(32),
        );
        let plan = parse_plan(&text).unwrap();
        assert_eq!(plan.genesis_total.to_string(), "1005307");
        assert_eq!(plan.reserve_genesis.to_string(), "1000000");
        assert_eq!(plan.epoch_blocks, Some(4));
        let mining = plan.mining.unwrap();
        assert_eq!(mining.initial_epoch_reward.to_string(), "1000");
        assert_eq!(mining.tail_epoch_reward.to_string(), "100");
        assert_eq!(mining.miner_share_bps, 7000);
    }

    #[test]
    fn a_chain_without_mining_has_no_emission_to_check() {
        let plan = parse_plan(r#"{"allocations": []}"#).unwrap();
        assert_eq!(plan.genesis_total, Amount::ZERO);
        assert!(plan.mining.is_none() && plan.epoch_blocks.is_none());
    }

    #[test]
    fn mining_without_staking_epochs_is_refused() {
        let text = r#"{"bft": {"mining": {"initial_epoch_reward": "1",
            "halving_epochs": 2, "miner_share_bps": 1}}}"#;
        assert_eq!(parse_plan(text), Err(GenesisError::MiningWithoutEpochs));
    }

    #[test]
    fn a_negative_or_fractional_amount_is_refused() {
        let text = format!(
            r#"{{"allocations": [{{"address": "{}", "balance": -1}}]}}"#,
            "00".repeat(32)
        );
        assert!(matches!(parse_plan(&text), Err(GenesisError::Amount(_))));
    }
}
