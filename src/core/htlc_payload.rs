//! The wire forms of the lattice HTLC transitions.
//!
//! Three actions, and the split between them is who bears a failure. A lock is
//! the sender's own transaction, so a bad one — no balance, an expiry already
//! past — is an error. A claim and a refund are not: they race each other at
//! the expiry boundary as a matter of course, and the loser of that race must
//! not void the block the winner is in. See `crate::state::htlc_exec`.
//!
//! Sizes: a lock carries a 4,448-byte commitment and a claim a 1,408-byte
//! opening. Both are smaller than the 11,165-byte hybrid signature on the same
//! transaction, so HTLC-L does not change what bounds a block.

use maya_htlc_lattice::{COMMITMENT_BYTES, Commitment, OPENING_BYTES, Opening};

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// A lock's identifier: `derive_lock_id(sender, nonce)`.
pub type LockId = [u8; 32];

/// Escrow `amount` for `recipient` under a commitment until `expiry_height`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtlcLock {
    /// Who a claim pays. Fixed here, never in the claim, so an opening copied
    /// out of a mempool can only pay the party the sender named.
    pub recipient: [u8; 32],
    /// Native base units to escrow.
    pub amount: u64,
    /// First height at which a claim is refused and a refund admitted.
    pub expiry_height: u64,
    /// What an opening must open. The same bytes on both chains of a swap.
    pub commitment: Commitment,
}

/// Claim a lock by publishing its opening.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtlcClaim {
    /// Which lock.
    pub lock_id: LockId,
    /// The short `(s, e)`.
    pub opening: Opening,
}

/// Return an expired lock's escrow to its sender.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtlcRefund {
    /// Which lock.
    pub lock_id: LockId,
}

impl HtlcLock {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.recipient);
        buf.extend_from_slice(&self.amount.to_le_bytes());
        buf.extend_from_slice(&self.expiry_height.to_le_bytes());
        buf.extend_from_slice(&self.commitment.encode());
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload or a commitment
    /// that is non-canonical or trivially openable.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            recipient: reader.read_array::<32>()?,
            amount: reader.read_u64()?,
            expiry_height: reader.read_u64()?,
            commitment: Commitment::decode(reader.read_slice(COMMITMENT_BYTES)?)
                .map_err(lattice)?,
        })
    }
}

impl HtlcClaim {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.lock_id);
        buf.extend_from_slice(&self.opening.encode());
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload or a
    /// non-canonical opening. An opening outside the noise bound has no
    /// encoding at all, so it fails here, before any state is read.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            lock_id: reader.read_array::<32>()?,
            opening: Opening::decode(reader.read_slice(OPENING_BYTES)?).map_err(lattice)?,
        })
    }
}

impl HtlcRefund {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.lock_id);
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            lock_id: reader.read_array::<32>()?,
        })
    }
}

/// Turns a lattice decode failure into a node error.
fn lattice(error: maya_htlc_lattice::Error) -> NodeError {
    NodeError::Decode(format!("htlc: {error}"))
}
