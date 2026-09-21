//! The wire forms of the HTLC transitions — hash locks and lattice locks.
//!
//! Three actions, and the split between them is who bears a failure. A lock is
//! the sender's own transaction, so a bad one — no balance, an expiry already
//! past — is an error. A claim and a refund are not: they race each other at
//! the expiry boundary as a matter of course, and the loser of that race must
//! not void the block the winner is in. See `crate::state::htlc_exec`.
//!
//! Sizes: a hash lock carries a 33-byte tagged digest and its claim a 33-byte
//! tagged preimage; a lattice lock a 4,449-byte commitment and its claim a
//! 1,409-byte opening. All are smaller than the 11,165-byte hybrid signature
//! on the same transaction, so neither changes what bounds a block.

use maya_htlc_lattice::{Lock, Unlock};

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
    /// What a claim must unlock. The same digest (or commitment) on both
    /// chains of a swap.
    pub lock: Lock,
}

/// Claim a lock by publishing what unlocks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtlcClaim {
    /// Which lock.
    pub lock_id: LockId,
    /// The preimage, or the short `(s, e)`.
    pub unlock: Unlock,
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
        self.lock.encode_into(buf);
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload, an unknown lock
    /// tag, or a commitment that is non-canonical or trivially openable.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let recipient = reader.read_array::<32>()?;
        let amount = reader.read_u64()?;
        let expiry_height = reader.read_u64()?;
        let (lock, used) = Lock::decode(reader.peek_remaining()).map_err(lattice)?;
        reader.read_slice(used)?;
        Ok(Self {
            recipient,
            amount,
            expiry_height,
            lock,
        })
    }
}

impl HtlcClaim {
    /// Appends the wire form.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.lock_id);
        self.unlock.encode_into(buf);
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for a truncated payload or a
    /// non-canonical opening. An opening outside the noise bound has no
    /// encoding at all, so it fails here, before any state is read.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let lock_id = reader.read_array::<32>()?;
        let (unlock, used) = Unlock::decode(reader.peek_remaining()).map_err(lattice)?;
        reader.read_slice(used)?;
        Ok(Self { lock_id, unlock })
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

/// Turns an HTLC decode failure into a node error.
fn lattice(error: maya_htlc_lattice::Error) -> NodeError {
    NodeError::Decode(format!("htlc: {error}"))
}
