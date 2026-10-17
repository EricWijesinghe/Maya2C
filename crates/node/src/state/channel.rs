//! On-chain payment channel records.
//!
//! ## Lifecycle
//!
//! ```text
//!            OpenChannel
//!                 |
//!               Open ----- CooperativeClose ----> Closed
//!                 |
//!           DisputeClose
//!                 |
//!             Disputed ---- PenaltyClaim -------> Closed  (all funds to victim)
//!                 |
//!         FinalizeDispute (after the deadline) --> Closed  (disputed balances)
//! ```
//!
//! A unilateral close does **not** pay out immediately. It records the claimed
//! state and opens a window in which the counterparty can prove that state was
//! revoked. Paying out at once would make fraud costless: publish a stale state
//! showing yourself richer, and take the money before anyone can object.
//!
//! ## Why only a commitment is stored
//!
//! Each submitted state carries a hash committing to its revocation secret. The
//! chain stores that one hash. A victim punishes fraud by supplying the
//! preimage, which they only possess because the closer handed it over when
//! advancing past that state. Storing the secrets themselves would grow
//! on-chain state without bound for no gain.

use crate::core::codec::{ByteReader, put_amount};
use crate::core::payload::ChannelId;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use primitive_types::U256;

/// Encoded size of a [`ChannelRecord`].
///
/// Capacity and the disputed balances are 256-bit (ADR-046), 32 bytes each.
pub const CHANNEL_RECORD_LEN: usize = 32 + 32 + 32 + 1 + 8 + 32 + 32 + 8 + 32 + 32 + 8;

/// Where a channel is in its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelStatus {
    /// Funded and usable off chain.
    Open,
    /// A unilateral close is pending; the dispute window is running.
    Disputed,
    /// Settled. Funds have been paid out and the record is inert.
    Closed,
}

impl ChannelStatus {
    fn tag(self) -> u8 {
        match self {
            Self::Open => 0,
            Self::Disputed => 1,
            Self::Closed => 2,
        }
    }

    fn from_tag(tag: u8) -> Result<Self> {
        match tag {
            0 => Ok(Self::Open),
            1 => Ok(Self::Disputed),
            2 => Ok(Self::Closed),
            other => Err(NodeError::Decode(format!("unknown channel status {other}"))),
        }
    }
}

/// A channel as the chain knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelRecord {
    /// Opener.
    pub party_a: Address,
    /// Counterparty.
    pub party_b: Address,
    /// Total escrowed value. Every valid state must account for exactly this.
    pub capacity: U256,
    /// Lifecycle position.
    pub status: ChannelStatus,
    /// Sequence number of the state submitted in a dispute.
    pub dispute_seq: u64,
    /// Party A's balance in the disputed state.
    pub dispute_balance_a: U256,
    /// Party B's balance in the disputed state.
    pub dispute_balance_b: U256,
    /// Height at and after which a dispute may be finalized.
    pub dispute_deadline: u64,
    /// Address that submitted the disputed state. Only the *other* party may
    /// claim a penalty — the closer proving its own state stale is meaningless.
    pub dispute_closer: Address,
    /// Commitment to the disputed state's revocation secret.
    pub dispute_commitment: [u8; 32],
    /// Blocks a dispute stays open.
    pub dispute_window: u64,
}

impl ChannelRecord {
    /// Creates a freshly funded channel.
    #[must_use]
    pub fn new(party_a: Address, party_b: Address, capacity: U256, dispute_window: u64) -> Self {
        Self {
            party_a,
            party_b,
            capacity,
            status: ChannelStatus::Open,
            dispute_seq: 0,
            dispute_balance_a: U256::zero(),
            dispute_balance_b: U256::zero(),
            dispute_deadline: 0,
            dispute_closer: [0u8; 32],
            dispute_commitment: [0u8; 32],
            dispute_window,
        }
    }

    /// Whether `address` participates in this channel.
    #[must_use]
    pub fn is_participant(&self, address: &Address) -> bool {
        &self.party_a == address || &self.party_b == address
    }

    /// The participant opposite `address`, if any.
    #[must_use]
    pub fn counterparty_of(&self, address: &Address) -> Option<Address> {
        if &self.party_a == address {
            Some(self.party_b)
        } else if &self.party_b == address {
            Some(self.party_a)
        } else {
            None
        }
    }

    /// Fixed-width encoding.
    #[must_use]
    pub fn encode(&self) -> [u8; CHANNEL_RECORD_LEN] {
        let mut buf = Vec::with_capacity(CHANNEL_RECORD_LEN);
        buf.extend_from_slice(&self.party_a);
        buf.extend_from_slice(&self.party_b);
        put_amount(&mut buf, &self.capacity);
        buf.push(self.status.tag());
        buf.extend_from_slice(&self.dispute_seq.to_le_bytes());
        put_amount(&mut buf, &self.dispute_balance_a);
        put_amount(&mut buf, &self.dispute_balance_b);
        buf.extend_from_slice(&self.dispute_deadline.to_le_bytes());
        buf.extend_from_slice(&self.dispute_closer);
        buf.extend_from_slice(&self.dispute_commitment);
        buf.extend_from_slice(&self.dispute_window.to_le_bytes());
        let mut out = [0u8; CHANNEL_RECORD_LEN];
        out.copy_from_slice(&buf);
        out
    }

    /// Decodes a stored record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::InvalidLength`] for the wrong size, or
    /// [`NodeError::Decode`] for an unknown status.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != CHANNEL_RECORD_LEN {
            return Err(NodeError::InvalidLength {
                what: "channel record",
                expected: CHANNEL_RECORD_LEN,
                actual: bytes.len(),
            });
        }

        let mut reader = ByteReader::new(bytes);
        let record = Self {
            party_a: reader.read_array::<32>()?,
            party_b: reader.read_array::<32>()?,
            capacity: reader.read_amount()?,
            status: ChannelStatus::from_tag(reader.read_u8()?)?,
            dispute_seq: reader.read_u64()?,
            dispute_balance_a: reader.read_amount()?,
            dispute_balance_b: reader.read_amount()?,
            dispute_deadline: reader.read_u64()?,
            dispute_closer: reader.read_array::<32>()?,
            dispute_commitment: reader.read_array::<32>()?,
            dispute_window: reader.read_u64()?,
        };
        reader.finish()?;
        Ok(record)
    }

    /// Leaf hash contributed to the channel Merkle root.
    #[must_use]
    pub fn leaf(&self, channel_id: &ChannelId) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_derive_key("maya-flash channel leaf v1");
        hasher.update(channel_id);
        hasher.update(&self.encode());
        *hasher.finalize().as_bytes()
    }
}
