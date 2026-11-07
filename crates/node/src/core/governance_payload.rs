//! Wire forms of the governance transactions.
//!
//! All six are fixed-width apart from a proposal's change list, so a decoder
//! cannot be told to allocate, and a transaction's size is a function of what
//! it does rather than of who sent it.
//!
//! ## Why a work claim carries a beneficiary rather than a signature
//!
//! Nothing in [`WorkClaim`] proves the sender mined anything, and it does not
//! need to. The claim is only valid inside a block, at most one per block, and
//! putting it there required the proof of work the block already carries. The
//! miner is the party that chose the block's contents, so the miner is the
//! party entitled to say who is credited.
//!
//! ## Why a ballot carries no weight
//!
//! The voter's weight is read from chain state at the height the vote lands —
//! their locked stake plus their decayed work credit. A ballot that carried its
//! own weight would be a ballot whose weight the voter chose.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::upgrades::SunsetSpec;
use crate::upgrade::FeatureSet;

/// Most rule changes one proposal may carry.
///
/// Mirrors [`maya_governance::params::MAX_CHANGES_PER_PROPOSAL`] so the decoder
/// refuses an over-large proposal before allocating for it, rather than
/// building it and having the state machine reject it afterwards.
pub const MAX_CHANGES: usize = maya_governance::params::MAX_CHANGES_PER_PROPOSAL;

/// Encoded size of one `(tag, value)` change.
pub const CHANGE_SIZE: usize = 2 + 32;

/// Most suite sunsets one schedule entry may carry: one per registered suite.
pub const MAX_SUNSETS: usize = 8;

/// Encoded size of one suite sunset: suite byte, sunset height, emergency flag.
const SUNSET_SIZE: usize = 1 + 8 + 1;

/// A protocol upgrade proposal (ADR-082): a rules epoch at a height, the
/// dark features it switches on, and the suites it retires.
///
/// A proposal carries either parameter changes, this, or both. The schedule is
/// written only if the proposal passes, and only with the lead the compiled
/// limits require (`maya_governance::limits::MIN_UPGRADE_LEAD_BLOCKS`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScheduleUpgrade {
    /// The rules epoch. Must exceed every scheduled version.
    pub version: u32,
    /// Height the epoch and its features take effect at.
    pub activation_height: u64,
    /// Dark features the epoch switches on, as a [`FeatureSet`] bitmask.
    pub features: FeatureSet,
    /// Suites retired at `activation_height`.
    pub sunsets: Vec<SunsetSpec>,
}

impl ScheduleUpgrade {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.version.to_le_bytes());
        buf.extend_from_slice(&self.activation_height.to_le_bytes());
        buf.extend_from_slice(&self.features.0.to_le_bytes());
        buf.extend_from_slice(&(self.sunsets.len() as u64).to_le_bytes());
        for sunset in &self.sunsets {
            buf.push(sunset.suite);
            buf.extend_from_slice(&sunset.sunset_height.to_le_bytes());
            buf.push(u8::from(sunset.emergency));
        }
    }

    /// Decodes an upgrade section.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] if the section is truncated or carries more than
    /// [`MAX_SUNSETS`] sunsets.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let version = u32::from_le_bytes(reader.read_array::<4>()?);
        let activation_height = reader.read_u64()?;
        let features = FeatureSet(reader.read_u64()?);
        let count = reader.read_collection_len(SUNSET_SIZE)?;
        if count > MAX_SUNSETS {
            return Err(NodeError::Decode(format!(
                "a schedule entry retiring {count} suites exceeds the maximum {MAX_SUNSETS}"
            )));
        }
        let mut sunsets = Vec::with_capacity(count);
        for _ in 0..count {
            sunsets.push(SunsetSpec {
                suite: reader.read_u8()?,
                sunset_height: reader.read_u64()?,
                emergency: reader.read_u8()? != 0,
            });
        }
        Ok(Self {
            version,
            activation_height,
            features,
            sunsets,
        })
    }
}

/// Crediting a block's work to an address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkClaim {
    /// Address whose work credit rises by this block's difficulty.
    pub beneficiary: Address,
}

/// Locking native coin for voting weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StakeLock {
    /// Units to lock.
    pub amount: primitive_types::U256,
    /// Earliest height the whole lock may be withdrawn at.
    ///
    /// A lock already in place is *extended* to the later of the two heights,
    /// never shortened. Shortening would let a voter reduce their own
    /// commitment after voting, which is the vote-then-sell attack with an
    /// extra step.
    pub unlock_height: u64,
}

/// Withdrawing locked coin once its height has passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StakeUnlock {
    /// Units to withdraw.
    pub amount: primitive_types::U256,
}

/// Opening a proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalSubmission {
    /// Blocks the proposal stays open for votes.
    pub voting_blocks: u64,
    /// Blocks between passing and becoming executable — the exit window.
    pub timelock_blocks: u64,
    /// Rule changes, as `(parameter tag, new value)`.
    pub changes: Vec<(u16, maya_dex::Amount)>,
    /// A protocol upgrade or suite sunset to schedule (ADR-082), if any.
    pub upgrade: Option<ScheduleUpgrade>,
}

/// Casting a vote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ballot {
    /// Proposal being voted on.
    pub proposal: [u8; 32],
    /// Tag of a [`maya_governance::tally::Choice`].
    pub choice: u8,
}

impl WorkClaim {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.beneficiary);
    }

    /// Decodes a claim.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            beneficiary: reader.read_array::<32>()?,
        })
    }
}

impl StakeLock {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        crate::core::codec::put_amount(buf, &self.amount);
        buf.extend_from_slice(&self.unlock_height.to_le_bytes());
    }

    /// Decodes a lock.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            amount: reader.read_amount()?,
            unlock_height: reader.read_u64()?,
        })
    }
}

impl StakeUnlock {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        crate::core::codec::put_amount(buf, &self.amount);
    }

    /// Decodes an unlock.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            amount: reader.read_amount()?,
        })
    }
}

impl ProposalSubmission {
    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.voting_blocks.to_le_bytes());
        buf.extend_from_slice(&self.timelock_blocks.to_le_bytes());
        buf.extend_from_slice(&(self.changes.len() as u64).to_le_bytes());
        for (tag, value) in &self.changes {
            buf.extend_from_slice(&tag.to_le_bytes());
            buf.extend_from_slice(&value.to_le_bytes());
        }
        match &self.upgrade {
            Some(upgrade) => {
                buf.push(1);
                upgrade.encode_into(buf);
            }
            None => buf.push(0),
        }
    }

    /// Decodes a proposal.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated, carries no
    /// changes, or carries more than [`MAX_CHANGES`].
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let voting_blocks = reader.read_u64()?;
        let timelock_blocks = reader.read_u64()?;

        let count = reader.read_collection_len(CHANGE_SIZE)?;
        if count > MAX_CHANGES {
            return Err(NodeError::Decode(format!(
                "a proposal carrying {count} changes exceeds the maximum {MAX_CHANGES}"
            )));
        }

        let mut changes = Vec::with_capacity(count);
        for _ in 0..count {
            let low = reader.read_u8()?;
            let high = reader.read_u8()?;
            changes.push((
                u16::from(low) | (u16::from(high) << 8),
                maya_dex::Amount::from_le_bytes(reader.read_array::<32>()?),
            ));
        }

        let upgrade = match reader.read_u8()? {
            0 => None,
            1 => Some(ScheduleUpgrade::decode(reader)?),
            flag => return Err(NodeError::Decode(format!("upgrade flag {flag}"))),
        };
        // A proposal that changes nothing and schedules nothing is a vote about
        // nothing, and it would still consume a deposit, a voting window, and
        // everyone's attention.
        if changes.is_empty() && upgrade.is_none() {
            return Err(NodeError::Decode(
                "a proposal must carry a change or a schedule entry".to_string(),
            ));
        }

        Ok(Self {
            voting_blocks,
            timelock_blocks,
            changes,
            upgrade,
        })
    }
}

impl Ballot {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.proposal);
        buf.push(self.choice);
    }

    /// Decodes a ballot.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            proposal: reader.read_array::<32>()?,
            choice: reader.read_u8()?,
        })
    }
}
