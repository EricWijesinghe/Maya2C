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

/// Most rule changes one proposal may carry.
///
/// Mirrors [`maya_governance::params::MAX_CHANGES_PER_PROPOSAL`] so the decoder
/// refuses an over-large proposal before allocating for it, rather than
/// building it and having the state machine reject it afterwards.
pub const MAX_CHANGES: usize = maya_governance::params::MAX_CHANGES_PER_PROPOSAL;

/// Encoded size of one `(tag, value)` change.
pub const CHANGE_SIZE: usize = 2 + 8;

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
    pub amount: u64,
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
    pub amount: u64,
}

/// Opening a proposal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalSubmission {
    /// Blocks the proposal stays open for votes.
    pub voting_blocks: u64,
    /// Blocks between passing and becoming executable — the exit window.
    pub timelock_blocks: u64,
    /// Rule changes, as `(parameter tag, new value)`.
    pub changes: Vec<(u16, u64)>,
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
        buf.extend_from_slice(&self.amount.to_le_bytes());
        buf.extend_from_slice(&self.unlock_height.to_le_bytes());
    }

    /// Decodes a lock.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            amount: reader.read_u64()?,
            unlock_height: reader.read_u64()?,
        })
    }
}

impl StakeUnlock {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.amount.to_le_bytes());
    }

    /// Decodes an unlock.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            amount: reader.read_u64()?,
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
        if count == 0 {
            // A proposal that changes nothing is a vote about nothing, and it
            // would still consume a deposit, a voting window, and everyone's
            // attention.
            return Err(NodeError::Decode(
                "a proposal must carry at least one change".to_string(),
            ));
        }
        if count > MAX_CHANGES {
            return Err(NodeError::Decode(format!(
                "a proposal carrying {count} changes exceeds the maximum {MAX_CHANGES}"
            )));
        }

        let mut changes = Vec::with_capacity(count);
        for _ in 0..count {
            let low = reader.read_u8()?;
            let high = reader.read_u8()?;
            changes.push((u16::from(low) | (u16::from(high) << 8), reader.read_u64()?));
        }

        Ok(Self {
            voting_blocks,
            timelock_blocks,
            changes,
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
