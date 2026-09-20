//! Hash Time-Locked Contracts.
//!
//! An HTLC conditionally reserves value inside a channel: the receiver claims it
//! by revealing a preimage before an expiry height, otherwise the sender
//! reclaims it afterwards. Chaining one across several channels with the *same*
//! hash lock is what makes a multi-hop payment atomic — revealing the preimage
//! to claim the last hop necessarily reveals it to every upstream hop.
//!
//! ## Expiry must decrease along the route
//!
//! Each hop's expiry is strictly earlier than the hop before it. If they were
//! equal, an intermediary could be left having paid downstream while its own
//! incoming HTLC expired in the same block — losing the amount outright. The
//! decreasing ladder guarantees every node has a window to claim upstream after
//! being claimed downstream.

use custom_l1_node::core::ChannelId;

/// Preimage that unlocks an HTLC.
pub type Preimage = [u8; 32];

/// Hash lock committing to a preimage.
pub type HashLock = [u8; 32];

/// Blocks of expiry margin each hop reserves for itself.
///
/// The value only has to exceed the time it can take to get a claim confirmed;
/// too small and an intermediary can be squeezed between its two HTLCs.
pub const HOP_EXPIRY_DELTA: u64 = 24;

/// Computes the hash lock for a preimage.
#[must_use]
pub fn hash_lock(preimage: &Preimage) -> HashLock {
    let mut hasher = blake3::Hasher::new_derive_key("maya-flash htlc hash lock v1");
    hasher.update(preimage);
    *hasher.finalize().as_bytes()
}

/// Which side of a channel offered an HTLC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Party A locked the value, payable to B.
    AtoB,
    /// Party B locked the value, payable to A.
    BtoA,
}

impl Direction {
    /// The opposite direction.
    #[must_use]
    pub fn reversed(self) -> Self {
        match self {
            Self::AtoB => Self::BtoA,
            Self::BtoA => Self::AtoB,
        }
    }
}

/// Value held in escrow inside a channel pending a preimage or expiry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Htlc {
    /// Identifier, unique within the channel.
    pub id: u64,
    /// Value reserved.
    pub amount: u64,
    /// Commitment to the unlocking preimage.
    pub hash_lock: HashLock,
    /// Block height at and after which the offerer may reclaim the value.
    pub expiry_height: u64,
    /// Which side offered it.
    pub direction: Direction,
}

impl Htlc {
    /// Whether `preimage` unlocks this HTLC.
    #[must_use]
    pub fn accepts(&self, preimage: &Preimage) -> bool {
        // Comparison is over the hash, not the preimage, so a wrong guess
        // reveals nothing beyond "wrong".
        hash_lock(preimage) == self.hash_lock
    }

    /// Whether the HTLC has expired at `height`.
    ///
    /// Expiry is inclusive: at exactly `expiry_height` the claim window has
    /// closed and only a refund is possible. Leaving it ambiguous would let a
    /// single block be both claimable and refundable.
    #[must_use]
    pub fn is_expired(&self, height: u64) -> bool {
        height >= self.expiry_height
    }

    /// Commitment bytes contributed to the channel state's HTLC root.
    #[must_use]
    pub fn commitment_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + 8 + 32 + 8 + 1);
        buf.extend_from_slice(&self.id.to_le_bytes());
        buf.extend_from_slice(&self.amount.to_le_bytes());
        buf.extend_from_slice(&self.hash_lock);
        buf.extend_from_slice(&self.expiry_height.to_le_bytes());
        buf.push(match self.direction {
            Direction::AtoB => 0,
            Direction::BtoA => 1,
        });
        buf
    }
}

/// Commitment over the pending HTLC set.
///
/// Order-sensitive by construction: callers keep the set sorted by id so the
/// root is a function of content rather than insertion order, and both parties
/// compute the same value.
#[must_use]
pub fn htlc_root(htlcs: &[Htlc]) -> [u8; 32] {
    if htlcs.is_empty() {
        return [0u8; 32];
    }

    let mut hasher = blake3::Hasher::new_derive_key("maya-flash htlc root v1");
    hasher.update(&(htlcs.len() as u64).to_le_bytes());
    for htlc in htlcs {
        hasher.update(&htlc.commitment_bytes());
    }
    *hasher.finalize().as_bytes()
}

/// One hop's HTLC parameters within a multi-hop payment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HopHtlc {
    /// Channel the HTLC is added to.
    pub channel_id: ChannelId,
    /// Value forwarded on this hop, including downstream fees.
    pub amount: u64,
    /// Shared across every hop of one payment.
    pub hash_lock: HashLock,
    /// Expiry for this hop; strictly greater than the next hop's.
    pub expiry_height: u64,
}
