//! The mining share transaction (ADR-044), and the work kind it and a work
//! claim carry.
//!
//! Carries only what the miner chose: the kind of work, the epoch and the
//! nonce. The payee is the transaction's sender, derived from the keys that
//! signed it, and the anchor and target come from committed state, so nothing a
//! miner writes can name a different payee or an easier target.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// What kind of work a share or a work claim is for (ADR-044, ADR-079).
///
/// A closed enum, like `SuiteId` (invariant 29): an undefined byte is a decode
/// error, never a fallback, because a kind some nodes know and others do not is
/// a fork. A new kind is a new variant shipped in a binary, refused by the
/// executor below the activation height of its own feature bit, which only a
/// scheduled upgrade sets (ADR-082). Neither governance nor genesis can add
/// one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum WorkKind {
    /// DAG proof of work (`docs/dag-pow.md`), the only kind at genesis.
    DagPow = 0,
}

impl WorkKind {
    /// The wire byte.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self as u8
    }

    /// Reads a kind byte.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for any byte that names no kind, or if the section
    /// is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        match reader.read_u8()? {
            0 => Ok(Self::DagPow),
            b => Err(NodeError::Decode(format!(
                "work kind {b} is not defined; kind 0 is DAG proof of work, and a new kind needs a scheduled upgrade"
            ))),
        }
    }
}

/// One proof-of-work share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MiningShare {
    /// The kind of work. [`WorkKind::DagPow`] is the only one.
    pub kind: WorkKind,
    /// The epoch the work was done for. Refused in any other epoch.
    pub epoch: u64,
    /// The nonce whose DAG digest meets the share target.
    pub nonce: u64,
}

impl MiningShare {
    /// Appends the fixed-width encoding: kind, epoch, nonce.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.push(self.kind.to_byte());
        buf.extend_from_slice(&self.epoch.to_le_bytes());
        buf.extend_from_slice(&self.nonce.to_le_bytes());
    }

    /// Decodes a share.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] if the section is truncated or the kind byte
    /// names no [`WorkKind`].
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            kind: WorkKind::decode(reader)?,
            epoch: reader.read_u64()?,
            nonce: reader.read_u64()?,
        })
    }
}
