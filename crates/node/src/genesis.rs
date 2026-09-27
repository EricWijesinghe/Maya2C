//! Genesis configuration.
//!
//! A testnet's genesis must be reproducible: every node has to derive an
//! identical genesis block and identical starting balances from the same file,
//! or they will disagree about the chain from block one and never converge.
//!
//! Reproducibility here rests on three things:
//!
//! - **Allocations are sorted by address** before hashing, so the JSON file's
//!   ordering — which a human editing it will not preserve — cannot change the
//!   state root.
//! - **The state root is computed from the config alone**, with the same Merkle
//!   construction the live chain uses. A node can therefore verify that what it
//!   wrote to disk matches what the config declared.
//! - **`chain_id` is folded into the genesis `prev_hash`**, so two networks with
//!   different ids can never share a genesis block, and a block mined for one
//!   cannot be replayed onto the other.

use serde::{Deserialize, Serialize};

use crate::core::{Block, BlockHeader};
use crate::crypto::pow::target_from_leading_zero_bits;
use crate::error::{NodeError, Result};
use crate::state::merkle::{account_leaf, merkle_root};
use maya_vrf::keys::VrfPublicKey;

use crate::oracle::registry::{OracleAuthority, OracleRegistry};
use crate::sealed::CommitteeRecord;
use crate::state::{Account, Address, StateDB};
use crate::upgrade::{ProtocolUpgrade, UpgradeSchedule};

/// A premined balance assigned at genesis.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Allocation {
    /// Hex-encoded 32-byte account address.
    pub address: String,
    /// Balance credited at genesis, in base units.
    pub balance: u64,
}

/// Everything needed to construct a network's first block and starting state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisConfig {
    /// Network identifier. Distinguishes otherwise identical testnets.
    pub chain_id: String,
    /// Genesis block timestamp, Unix seconds.
    pub timestamp: u64,
    /// Initial difficulty, as required leading zero bits.
    pub difficulty_bits: u32,
    /// Easiest target the network will ever retarget to, in leading zero bits.
    ///
    /// Must be no greater than `difficulty_bits`: the floor cannot be harder
    /// than the starting difficulty, or genesis would be invalid under the
    /// network's own rules.
    pub pow_limit_bits: u32,
    /// Premined balances.
    pub allocations: Vec<Allocation>,
    /// The oracle authority set, if this network runs one.
    ///
    /// Optional, and absent by default, so that a chain configured without it
    /// has no `o:` records and therefore the state root it would have had
    /// before the oracle existed. Introducing a trusted party has to be a
    /// decision somebody wrote down, not a consequence of upgrading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oracle: Option<OracleGenesis>,
    /// The sealed-mempool encryption committee, if this network runs one.
    ///
    /// Optional and absent by default, for the same reason the oracle is: a
    /// chain configured without it has no `m:` records and therefore the state
    /// root it would have had before the sealed mempool existed.
    ///
    /// It is also where the scheme's trusted setup lives. Whoever generated
    /// these keys held the committee secret at the moment they did. See
    /// `docs/sealed-mempool.md` and [`maya_mev::Committee::generate`] for what
    /// a compromised setup costs — which is the MEV protection, and nothing
    /// else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<SealedGenesis>,
    /// The DAO treasury, if this network funds one at genesis.
    ///
    /// Optional and absent by default, for the same reason the oracle and the
    /// sealed committee are: a chain configured without it has no treasury
    /// account, and therefore the state root it would have had before the
    /// treasury existed. See [`TreasuryGenesis`] for why it is not simply an
    /// allocation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub treasury: Option<TreasuryGenesis>,
    /// Scheduled protocol upgrades (`crate::upgrade`). Empty by default and
    /// hashed into nothing, so every existing genesis keeps its id and root;
    /// a node reaching a scheduled version it does not implement halts with
    /// "upgrade required before height H" instead of forking.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocol_upgrades: Vec<ProtocolUpgrade>,
    /// The DAG-BFT committee, on a network ordered by DAG-BFT (ADR-015,
    /// ADR-027). Absent means proof of work, which is what every genesis file
    /// written before this field meant; present, it is committed into the
    /// genesis id (`chain_id_commitment`), so two operators who disagree about
    /// the committee disagree about block zero rather than at round one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bft: Option<BftGenesis>,
}

/// The genesis committee of a DAG-BFT network.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BftGenesis {
    /// Hex ML-DSA-65 verifying keys, in committee order: index `i` here is
    /// validator id `i` in every vertex and vote. Order is consensus.
    pub validators: Vec<String>,
    /// How long a validator waits for a round's anchor before advancing
    /// without it, in milliseconds. Consensus: it shapes the DAG, so every
    /// validator must use the same value.
    #[serde(default = "BftGenesis::default_anchor_timeout_ms")]
    pub anchor_timeout_ms: u64,
    /// Most transactions per vertex.
    #[serde(default = "BftGenesis::default_batch_size")]
    pub batch_size: usize,
    /// Least milliseconds between one validator's proposals unless a full
    /// batch waits: what keeps an idle chain from minting empty blocks as
    /// fast as the network turns rounds (ADR-027).
    #[serde(default = "BftGenesis::default_round_interval_ms")]
    pub round_interval_ms: u64,
    /// Staking (ADR-028). Absent: the genesis committee orders the chain for
    /// ever and no `k:` record exists. Present: the genesis validators are
    /// bonded at genesis and later committees come from stake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staking: Option<StakingGenesis>,
    /// The fee market (ADR-029). Absent: transactions pay nothing, as on
    /// every chain before this field. Present: charged from block 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fees: Option<FeesGenesis>,
}

/// Genesis fee-market parameters, checked against `maya-fee-market`'s limits.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeesGenesis {
    /// Base fee at block 1, per serialized byte.
    pub initial_base_fee: u64,
    /// Floor for the base fee.
    pub min_base_fee: u64,
    /// Block size the base fee steers toward, in bytes.
    pub target_block_bytes: u64,
    /// The base fee moves by at most `1 / change_denominator` per block.
    pub change_denominator: u64,
}

impl FeesGenesis {
    /// The initial fee record.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] if the parameters are outside the fee market's
    /// compiled-in bounds.
    pub fn record(&self) -> Result<crate::state::fees::FeeRecord> {
        let config = maya_fee_market::FeeConfig {
            activation_height: 1,
            target_block_bytes: self.target_block_bytes,
            min_base_fee: self.min_base_fee,
            change_denominator: self.change_denominator,
            treasury_bps: 0,
            initial_base_fee: self.initial_base_fee,
            neural_activation_height: u64::MAX,
        };
        config
            .validate()
            .map_err(|e| NodeError::Decode(format!("bft.fees: {e:?}")))?;
        Ok(crate::state::fees::FeeRecord {
            base_fee: self.initial_base_fee,
            min_base_fee: self.min_base_fee,
            target_block_bytes: self.target_block_bytes,
            change_denominator: self.change_denominator,
        })
    }
}

/// Genesis staking: epoch length, parameters, and the genesis bonds.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct StakingGenesis {
    /// Blocks per epoch.
    pub epoch_blocks: u64,
    /// One bond per genesis validator, in `validators` order.
    pub bonds: Vec<GenesisBond>,
    /// Least self bond. Defaults to the devnet value.
    #[serde(default = "StakingGenesis::default_min_self_bond")]
    pub min_self_bond: u64,
    /// Largest committee. Defaults to the devnet value.
    #[serde(default = "StakingGenesis::default_max_validators")]
    pub max_validators: u16,
}

/// One genesis validator's bond.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisBond {
    /// Hex address of the operator: pays nothing at genesis, receives
    /// commission and rewards, and alone may add to or unbond the bond.
    pub operator: String,
    /// The bond. Counts toward total supply.
    pub bond: u64,
    /// Commission, basis points.
    #[serde(default)]
    pub commission_bps: u16,
}

impl StakingGenesis {
    const fn default_min_self_bond() -> u64 {
        maya_staking::Params::DEVNET.min_self_bond
    }

    const fn default_max_validators() -> u16 {
        maya_staking::Params::DEVNET.max_validators
    }

    /// Staking parameters.
    #[must_use]
    pub fn params(&self) -> maya_staking::Params {
        maya_staking::Params {
            min_self_bond: self.min_self_bond,
            max_validators: self.max_validators,
            ..maya_staking::Params::DEVNET
        }
    }

    /// Sum of the genesis bonds.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] on overflow.
    pub fn bonded(&self) -> Result<u64> {
        self.bonds.iter().try_fold(0u64, |acc, b| {
            acc.checked_add(b.bond)
                .ok_or_else(|| NodeError::Decode("genesis bonds overflow u64".to_string()))
        })
    }
}

impl BftGenesis {
    const fn default_anchor_timeout_ms() -> u64 {
        1_000
    }

    const fn default_batch_size() -> usize {
        500
    }

    const fn default_round_interval_ms() -> u64 {
        500
    }

    /// The decoded committee keys, in order.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for an empty committee, a malformed key, or the
    /// same key twice (one operator holding two votes).
    pub fn verifying_keys(&self) -> Result<Vec<crate::crypto::keys::VerifyingKey>> {
        if self.validators.is_empty() {
            return Err(NodeError::Decode("bft.validators must not be empty".into()));
        }
        if self.validators.len() > usize::from(u16::MAX) {
            return Err(NodeError::Decode("bft.validators exceeds u16".into()));
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut keys = Vec::with_capacity(self.validators.len());
        for (i, encoded) in self.validators.iter().enumerate() {
            let bytes = hex::decode(encoded)
                .map_err(|e| NodeError::Decode(format!("bft.validators[{i}]: {e}")))?;
            let array: [u8; crate::crypto::keys::PUBLIC_KEY_LEN] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| NodeError::Decode(format!("bft.validators[{i}]: wrong key length")))?;
            if !seen.insert(array) {
                return Err(NodeError::Decode(format!(
                    "bft.validators[{i}] duplicates an earlier key"
                )));
            }
            keys.push(crate::crypto::keys::VerifyingKey::from_bytes(&array)?);
        }
        Ok(keys)
    }
}

/// The DAO treasury as it is funded at genesis.
///
/// # Why this is not just another allocation
///
/// Mechanically it becomes one — the treasury is an account with a balance,
/// and [`GenesisConfig::accounts`] returns it alongside the rest. Naming it
/// separately buys three things an anonymous line in `allocations` does not:
///
/// 1. **It is visible in a diff.** An operator comparing two genesis files
///    sees `treasury` change; they do not necessarily notice that one of forty
///    hex addresses now holds a different number.
/// 2. **It cannot silently collide.** [`GenesisConfig::validate`] refuses a
///    treasury address that also appears in `allocations`, because two entries
///    for one address would make the balance depend on which was applied last.
/// 3. **It carries its share.** `share_bps` is checked against the total
///    supply, so a treasury that was meant to be 10% and is actually 100% is a
///    validation error rather than a discovery.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TreasuryGenesis {
    /// Hex-encoded 32-byte address the treasury balance is credited to.
    pub address: String,
    /// Units credited to that address.
    pub balance: u64,
    /// The share of total genesis supply this is intended to be, in basis
    /// points.
    ///
    /// Declared and then checked, not derived. A number the author wrote down
    /// and a number the file actually implies are different things, and the
    /// gap between them is exactly the mistake worth catching before launch.
    pub share_bps: u16,
}

/// Basis points in a whole. 10 000 bps = 100%.
pub const BPS_DENOMINATOR: u64 = 10_000;

/// Largest share of genesis supply a treasury may hold, in basis points.
///
/// # Why there is a ceiling at all
///
/// A treasury holding most of the supply is not a treasury, it is the chain.
/// 5 000 bps — half — is well above any allocation a launch would defend and
/// far below the point where the distribution stops meaning anything, and it
/// turns "the decimal moved" into a validation error rather than a governance
/// crisis discovered after genesis is immutable.
///
/// This is a *genesis* bound, deliberately separate from the governance limits
/// in `crates/governance/src/limits.rs`: those constrain what a proposal may later do,
/// and cannot constrain what the chain started as.
pub const MAX_TREASURY_SHARE_BPS: u16 = 5_000;

/// The sealed-mempool committee as it is configured at genesis.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SealedGenesis {
    /// Decryption shares required to open an envelope.
    pub threshold: u16,
    /// Hex-encoded compressed ristretto255 aggregate encryption key.
    pub encryption_key: String,
    /// The committee members.
    pub members: Vec<GenesisCommitteeMember>,
}

/// One genesis committee member.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisCommitteeMember {
    /// The member's Shamir evaluation point, in `1..=members`.
    ///
    /// Never zero: index zero is where the committee secret itself sits in the
    /// polynomial, so a member issued it would be issued the whole key.
    pub index: u16,
    /// Hex-encoded compressed ristretto255 verification key.
    pub key: String,
}

/// The oracle authority set as it is configured at genesis.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OracleGenesis {
    /// Signatures required to move a feed or rotate this set.
    pub quorum: u8,
    /// The authorities.
    pub authorities: Vec<GenesisAuthority>,
}

/// One genesis oracle authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisAuthority {
    /// Hex-encoded 32-byte account address whose signature counts.
    pub address: String,
    /// Hex-encoded VRF public key: one scheme byte then the compressed point.
    pub vrf_key: String,
}

impl OracleGenesis {
    /// Builds the registry this configuration describes.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a malformed address or key, or
    /// [`NodeError::InvalidOracleRegistry`] if the set does not satisfy the
    /// registry's own rules — which are checked here rather than trusted,
    /// because a genesis file is a text file somebody edited.
    pub fn registry(&self) -> Result<OracleRegistry> {
        let authorities = self
            .authorities
            .iter()
            .map(|authority| {
                let address = decode_address(&authority.address)?;
                let raw = hex::decode(&authority.vrf_key)
                    .map_err(|e| NodeError::Decode(format!("invalid genesis VRF key hex: {e}")))?;
                let vrf_key = VrfPublicKey::decode(&raw)
                    .map_err(|e| NodeError::Decode(format!("invalid genesis VRF key: {e}")))?;
                Ok(OracleAuthority { address, vrf_key })
            })
            .collect::<Result<Vec<_>>>()?;

        OracleRegistry::new(authorities, self.quorum, 0)
    }
}

impl SealedGenesis {
    /// Builds the committee record this configuration describes.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for malformed hex, or
    /// [`NodeError::SealedCommittee`] if the committee does not satisfy its own
    /// rules — checked here rather than trusted, because a genesis file is a
    /// text file somebody edited, and a committee whose threshold exceeds its
    /// membership would make every sealed transaction on the chain expire.
    pub fn record(&self) -> Result<CommitteeRecord> {
        let members = self
            .members
            .iter()
            .map(|member| {
                Ok((
                    member.index,
                    decode_point("committee member key", &member.key)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;

        let record = CommitteeRecord {
            encryption_key: decode_point("committee encryption key", &self.encryption_key)?,
            threshold: self.threshold,
            members,
        };
        record.validate()?;
        Ok(record)
    }
}

/// Decodes a hex-encoded 32-byte curve point from a genesis file.
fn decode_point(what: &str, encoded: &str) -> Result<[u8; 32]> {
    let raw = hex::decode(encoded)
        .map_err(|e| NodeError::Decode(format!("invalid genesis {what} hex: {e}")))?;
    raw.as_slice()
        .try_into()
        .map_err(|_| NodeError::Decode(format!("genesis {what} is not 32 bytes")))
}

impl GenesisConfig {
    /// The validated upgrade schedule.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] if the upgrades are out of order.
    pub fn upgrade_schedule(&self) -> Result<UpgradeSchedule> {
        UpgradeSchedule::new(self.protocol_upgrades.clone())
    }

    /// Validates the configuration.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the chain id is empty, the difficulty
    /// floor is harder than the starting difficulty, an address is malformed,
    /// an address is duplicated, or the total allocation overflows `u64`.
    pub fn validate(&self) -> Result<()> {
        if self.chain_id.is_empty() {
            return Err(NodeError::Decode("chain_id must not be empty".to_string()));
        }
        self.upgrade_schedule()?;

        if self.pow_limit_bits > self.difficulty_bits {
            return Err(NodeError::Decode(format!(
                "pow_limit_bits ({}) is harder than difficulty_bits ({}); \
                 genesis would violate the network's own difficulty floor",
                self.pow_limit_bits, self.difficulty_bits
            )));
        }

        if self.difficulty_bits >= 256 {
            return Err(NodeError::Decode(
                "difficulty_bits must be below 256".to_string(),
            ));
        }

        let mut seen = std::collections::BTreeSet::new();
        let mut total: u64 = 0;
        for allocation in &self.allocations {
            let address = decode_address(&allocation.address)?;
            if !seen.insert(address) {
                // Two entries for one address would make the resulting balance
                // depend on iteration order.
                return Err(NodeError::Decode(format!(
                    "duplicate allocation for address {}",
                    allocation.address
                )));
            }
            total = total
                .checked_add(allocation.balance)
                .ok_or_else(|| NodeError::Decode("total allocation overflows u64".to_string()))?;
        }

        if let Some(treasury) = &self.treasury {
            let address = decode_address(&treasury.address)?;

            // Checked against the same `seen` set the allocations built, so a
            // treasury that duplicates an allocation is caught here rather
            // than producing a balance that depends on application order.
            if !seen.insert(address) {
                return Err(NodeError::Decode(format!(
                    "treasury address {} is also a plain allocation; one address \
                     must appear once",
                    treasury.address
                )));
            }

            if treasury.share_bps > MAX_TREASURY_SHARE_BPS {
                return Err(NodeError::Decode(format!(
                    "treasury share of {} bps exceeds the {MAX_TREASURY_SHARE_BPS} bps ceiling",
                    treasury.share_bps
                )));
            }

            total = total.checked_add(treasury.balance).ok_or_else(|| {
                NodeError::Decode("total supply overflows u64 with the treasury".to_string())
            })?;

            if total == 0 {
                return Err(NodeError::Decode(
                    "a treasury cannot be funded on a chain with no supply".to_string(),
                ));
            }

            // The declared share checked against the implied one. Integer
            // arithmetic throughout: a floating-point comparison here would
            // make validation depend on rounding, and a genesis file that
            // validated on one machine and not another is worse than one that
            // fails everywhere.
            let implied_bps =
                (u128::from(treasury.balance) * u128::from(BPS_DENOMINATOR)) / u128::from(total);
            if implied_bps != u128::from(treasury.share_bps) {
                return Err(NodeError::Decode(format!(
                    "treasury declares {} bps of supply but holds {implied_bps} bps \
                     ({} of {total} units)",
                    treasury.share_bps, treasury.balance
                )));
            }
        }

        Ok(())
    }

    /// Total units this genesis issues, treasury included.
    ///
    /// # Errors
    ///
    /// Propagates validation failures.
    pub fn total_supply(&self) -> Result<u64> {
        self.validate()?;
        let mut total: u64 = 0;
        for allocation in &self.allocations {
            total = total
                .checked_add(allocation.balance)
                .ok_or_else(|| NodeError::Decode("total allocation overflows u64".to_string()))?;
        }
        if let Some(treasury) = &self.treasury {
            total = total.checked_add(treasury.balance).ok_or_else(|| {
                NodeError::Decode("total supply overflows u64 with the treasury".to_string())
            })?;
        }
        if let Some(staking) = self.bft.as_ref().and_then(|b| b.staking.as_ref()) {
            total = total.checked_add(staking.bonded()?).ok_or_else(|| {
                NodeError::Decode("total supply overflows u64 with genesis bonds".to_string())
            })?;
        }
        Ok(total)
    }

    /// Allocations as decoded accounts, sorted by address.
    ///
    /// # Errors
    ///
    /// Propagates validation failures.
    pub fn accounts(&self) -> Result<Vec<(Address, Account)>> {
        self.validate()?;

        let mut accounts: Vec<(Address, Account)> = self
            .allocations
            .iter()
            .map(|allocation| {
                Ok((
                    decode_address(&allocation.address)?,
                    Account {
                        balance: allocation.balance,
                        nonce: 0,
                    },
                ))
            })
            .collect::<Result<Vec<_>>>()?;

        // The treasury is an account like any other once validation has
        // confirmed it does not collide with one. Appended before the sort, so
        // it lands in address order rather than at the end — the state root
        // must not depend on where in the file the treasury was declared.
        if let Some(treasury) = &self.treasury {
            accounts.push((
                decode_address(&treasury.address)?,
                Account {
                    balance: treasury.balance,
                    nonce: 0,
                },
            ));
        }

        // Sorting is what makes the state root independent of file ordering.
        accounts.sort_by_key(|(address, _)| *address);
        Ok(accounts)
    }

    /// The Merkle state root implied by the allocations.
    ///
    /// Computed without touching a database, using the same construction as
    /// live state, so a node can check its seeded database against the config.
    ///
    /// # Errors
    ///
    /// Propagates validation failures.
    pub fn state_root(&self) -> Result<[u8; 32]> {
        let leaves: Vec<[u8; 32]> = self
            .accounts()?
            .iter()
            .map(|(address, account)| account_leaf(address, account))
            .collect();
        let accounts = merkle_root(&leaves);
        // The staking layer is the only other layer a genesis can hold that
        // this function knows how to build; it folds in exactly as
        // `StateDB::state_layers` would fold the seeded records.
        let records = self.staking_records()?;
        if records.is_empty() {
            return Ok(accounts);
        }
        let leaves: Vec<[u8; 32]> = records
            .iter()
            .map(|(k, v)| crate::state::db::record_leaf((k, v)))
            .collect();
        Ok(crate::state::proof::StateLayer::Staking.fold(&accounts, &merkle_root(&leaves)))
    }

    /// The `k:` records a staking genesis seeds, in key order: the staking
    /// state with every genesis validator bonded and forming the epoch-0
    /// committee in `validators` order, each key, and committee 0.
    ///
    /// # Errors
    ///
    /// A malformed key or operator, a bond count that does not match the
    /// validator count, or a bond the staking rules refuse.
    pub fn staking_records(&self) -> Result<std::collections::BTreeMap<Vec<u8>, Vec<u8>>> {
        use crate::state::staking::{
            STATE_KEY, StakingRecord, committee_record, encode_committee, key_record, validator_id,
        };
        let mut records = std::collections::BTreeMap::new();
        let Some(bft) = &self.bft else {
            return Ok(records);
        };
        if let Some(fees) = &bft.fees {
            records.insert(
                crate::state::fees::FEE_KEY.to_vec(),
                fees.record()?.encode(),
            );
        }
        let Some(staking) = &bft.staking else {
            return Ok(records);
        };
        if staking.bonds.len() != bft.validators.len() {
            return Err(NodeError::Decode(
                "bft.staking.bonds must name one bond per validator".to_string(),
            ));
        }
        if staking.epoch_blocks == 0 {
            return Err(NodeError::Decode(
                "bft.staking.epoch_blocks must be positive".into(),
            ));
        }
        let mut state = maya_staking::Staking::new(staking.params())
            .map_err(|e| NodeError::Decode(format!("bft.staking params: {e:?}")))?;
        let mut active = Vec::new();
        for (key, bond) in bft.verifying_keys()?.iter().zip(&staking.bonds) {
            let encoded = key.to_bytes();
            let id = validator_id(&encoded);
            let operator = decode_address(&bond.operator)?;
            state
                .register(operator, id, bond.bond, bond.commission_bps)
                .map_err(|e| NodeError::Decode(format!("genesis bond refused: {e:?}")))?;
            records.insert(key_record(&id), encoded.to_vec());
            active.push(id);
        }
        state.active.clone_from(&active);
        let record = StakingRecord {
            staking: state,
            epoch_blocks: staking.epoch_blocks,
            last_round: 0,
            expected: std::collections::BTreeMap::new(),
            authored: std::collections::BTreeMap::new(),
        };
        records.insert(STATE_KEY.to_vec(), record.encode());
        records.insert(committee_record(0), encode_committee(&active));
        Ok(records)
    }

    /// The target genesis and its immediate descendants must satisfy.
    #[must_use]
    pub fn difficulty_target(&self) -> [u8; 32] {
        target_from_leading_zero_bits(self.difficulty_bits)
    }

    /// The network's difficulty floor.
    #[must_use]
    pub fn pow_limit(&self) -> [u8; 32] {
        target_from_leading_zero_bits(self.pow_limit_bits)
    }

    /// Domain-separated commitment to the chain id, used as genesis `prev_hash`.
    ///
    /// Genesis has no parent, so the field is free. Binding the chain id here
    /// makes every network's genesis hash distinct.
    ///
    /// Bumped to v2 for the move to hybrid signing. That change replaced the
    /// address derivation, so every allocation in a genesis file names a
    /// different account than the same hex string named before — a node running
    /// the old rules and a node running these would disagree about who owns the
    /// premine while agreeing on the genesis hash. Rotating the domain makes
    /// the two chains visibly distinct from block zero instead.
    #[must_use]
    pub fn chain_id_commitment(&self) -> [u8; 32] {
        let chain = blake3::derive_key(
            "custom-l1-node genesis chain id v2",
            self.chain_id.as_bytes(),
        );
        let Some(bft) = &self.bft else {
            return chain;
        };
        // Only when a committee is configured, so every proof-of-work genesis
        // keeps the id it always had.
        let mut h = blake3::Hasher::new_derive_key("maya2c genesis bft committee v1");
        h.update(&chain);
        h.update(&bft.anchor_timeout_ms.to_le_bytes());
        h.update(&(bft.batch_size as u64).to_le_bytes());
        h.update(&bft.round_interval_ms.to_le_bytes());
        if let Some(fees) = &bft.fees {
            h.update(b"fees");
            for v in [
                fees.initial_base_fee,
                fees.min_base_fee,
                fees.target_block_bytes,
                fees.change_denominator,
            ] {
                h.update(&v.to_le_bytes());
            }
        }
        if let Some(staking) = &bft.staking {
            h.update(b"staking");
            h.update(&staking.epoch_blocks.to_le_bytes());
            h.update(&staking.min_self_bond.to_le_bytes());
            h.update(&staking.max_validators.to_le_bytes());
            for bond in &staking.bonds {
                h.update(bond.operator.to_ascii_lowercase().as_bytes());
                h.update(&bond.bond.to_le_bytes());
                h.update(&bond.commission_bps.to_le_bytes());
            }
        }
        h.update(&(bft.validators.len() as u64).to_le_bytes());
        for key in &bft.validators {
            h.update(key.to_ascii_lowercase().as_bytes());
        }
        *h.finalize().as_bytes()
    }

    /// Builds the genesis block.
    ///
    /// # Errors
    ///
    /// Propagates validation failures.
    pub fn genesis_block(&self) -> Result<Block> {
        Ok(Block::new(
            BlockHeader {
                prev_hash: self.chain_id_commitment(),
                state_root: self.state_root()?,
                timestamp: self.timestamp,
                nonce: 0,
                difficulty_target: self.difficulty_target(),
                // Set by `Block::new`: the root of no transactions.
                tx_root: [0; 32],
            },
            Vec::new(),
        ))
    }

    /// Writes the allocations into `state` and verifies the resulting root.
    ///
    /// Idempotent: re-seeding an already-seeded database writes the same values
    /// and leaves the root unchanged, so a node restart is safe.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::StateRootMismatch`] if the seeded database does not
    /// reproduce the config's declared state root, which means the two have
    /// diverged and the node must not start.
    pub fn seed_state(&self, state: &StateDB) -> Result<[u8; 32]> {
        for (address, account) in self.accounts()? {
            state.put_account(&address, &account)?;
        }

        // The oracle is optional, and a chain configured without one has no
        // authority set, no beacon, and therefore no `o:` records — which means
        // the state root below is exactly what it would have been before this
        // subsystem existed. A network that does not want a trusted party does
        // not get one by upgrading.
        if let Some(oracle) = &self.oracle {
            state.seed_oracle(&oracle.registry()?, &self.chain_id)?;
        }

        // Likewise optional, and likewise a decision rather than a default: a
        // committee is a set of parties who can read the mempool before anyone
        // else, and installing one has to be something a network chose.
        if let Some(sealed) = &self.sealed {
            state.seed_sealed_committee(&sealed.record()?)?;
        }

        // Staking, when configured: bonds held by the module, not accounts.
        for (key, value) in self.staking_records()? {
            state.raw_put(&key, &value)?;
        }

        let expected = self.state_root()?;
        let actual = state.state_root()?;
        if actual != expected {
            return Err(NodeError::StateRootMismatch {
                expected: hex::encode(expected),
                actual: hex::encode(actual),
            });
        }

        Ok(actual)
    }

    /// Parses a configuration from JSON.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the JSON is malformed or invalid.
    pub fn from_json(json: &str) -> Result<Self> {
        let config: Self = serde_json::from_str(json)
            .map_err(|e| NodeError::Decode(format!("invalid genesis JSON: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Serializes to pretty-printed JSON with a trailing newline.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if serialization fails.
    pub fn to_json(&self) -> Result<String> {
        let mut json = serde_json::to_string_pretty(self)
            .map_err(|e| NodeError::Decode(format!("serializing genesis: {e}")))?;
        json.push('\n');
        Ok(json)
    }
}

/// Decodes a hex address, tolerating an optional `0x` prefix.
///
/// # Errors
///
/// Returns [`NodeError::Decode`] if the value is not 32 bytes of hex.
pub fn decode_address(value: &str) -> Result<Address> {
    let bytes = hex::decode(value.trim_start_matches("0x"))
        .map_err(|e| NodeError::Decode(format!("address {value} is not valid hex: {e}")))?;
    <Address>::try_from(bytes.as_slice()).map_err(|_| {
        NodeError::Decode(format!(
            "address {value} must be 32 bytes, got {}",
            bytes.len()
        ))
    })
}
