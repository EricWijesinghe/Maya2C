//! The mining share transaction (ADR-044).
//!
//! Carries only what the miner chose: the epoch and the nonce. The payee is
//! the transaction's sender, derived from the keys that signed it, and the
//! anchor and target come from committed state, so nothing a miner writes can
//! name a different payee or an easier target.

use crate::core::codec::ByteReader;
use crate::error::Result;

/// One proof-of-work share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MiningShare {
    /// The epoch the work was done for. Refused in any other epoch.
    pub epoch: u64,
    /// The nonce whose DAG digest meets the share target.
    pub nonce: u64,
}

impl MiningShare {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.epoch.to_le_bytes());
        buf.extend_from_slice(&self.nonce.to_le_bytes());
    }

    /// Decodes a share.
    ///
    /// # Errors
    ///
    /// [`crate::error::NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            epoch: reader.read_u64()?,
            nonce: reader.read_u64()?,
        })
    }
}
