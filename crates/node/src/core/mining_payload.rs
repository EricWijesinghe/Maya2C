//! The mining share transaction (ADR-044), and the work kind it and a work
//! claim carry.
//!
//! Carries only what the miner chose: the kind of work, the epoch and the
//! nonces. The payee is the transaction's sender, derived from the keys that
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

/// A batch of proof-of-work shares under one signature (MINE-03, ADR-044
/// amendment 2026-11-24), or, with no nonces, a claim of the sender's credit
/// for a closed epoch.
///
/// One PQ signature covers every nonce, so the bytes a share costs on chain
/// are its 8-byte nonce plus a fixed overhead split across the batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MiningShare {
    /// The kind of work. [`WorkKind::DagPow`] is the only one.
    pub kind: WorkKind,
    /// The epoch the work was done for, or, for a claim, the closed epoch
    /// whose credit is claimed.
    pub epoch: u64,
    /// The nonces whose DAG digests meet the share target, strictly
    /// ascending, so a batch has one encoding and cannot name a nonce twice.
    /// Empty: a claim.
    pub nonces: Vec<u64>,
}

impl MiningShare {
    /// The most shares a miner puts in one batch when a block takes
    /// `max_shares_per_block`: a quarter of it, at least one.
    ///
    /// Miner policy, not consensus: any batch up to the block cap is valid.
    /// A block admits a batch whole or not at all, so a batch as large as the
    /// cap fits only a block with no other share in it; once blocks are busy
    /// the small batches take the slots and the miner with the most hashrate
    /// is the one left out. Batches of a quarter fit beside other miners' work
    /// (`a_miner_with_more_finds_than_a_busy_block_has_room_for_is_still_credited`).
    #[must_use]
    pub const fn batch_limit(max_shares_per_block: u64) -> u64 {
        let quarter = max_shares_per_block / 4;
        if quarter == 0 { 1 } else { quarter }
    }

    /// One share: the common case, and what a miner with one find sends.
    #[must_use]
    pub fn single(epoch: u64, nonce: u64) -> Self {
        Self {
            kind: WorkKind::DagPow,
            epoch,
            nonces: vec![nonce],
        }
    }

    /// Appends the encoding: kind, epoch, nonce count (u32), nonces.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.push(self.kind.to_byte());
        buf.extend_from_slice(&self.epoch.to_le_bytes());
        // A decoded batch fits the 4 MiB frame, so its count fits u32.
        let count = u32::try_from(self.nonces.len()).unwrap_or(u32::MAX);
        buf.extend_from_slice(&count.to_le_bytes());
        for nonce in &self.nonces {
            buf.extend_from_slice(&nonce.to_le_bytes());
        }
    }

    /// Decodes a batch.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] if the section is truncated, the kind byte names
    /// no [`WorkKind`], the count claims more nonces than the bytes left, or
    /// the nonces are not strictly ascending.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let kind = WorkKind::decode(reader)?;
        let epoch = reader.read_u64()?;
        let count = usize::try_from(reader.read_u32()?)
            .map_err(|_| NodeError::Decode("share count".into()))?;
        // Checked before allocating: a count is only as good as its bytes.
        if count.saturating_mul(8) > reader.remaining() {
            return Err(NodeError::Decode("share count exceeds the payload".into()));
        }
        let mut nonces = Vec::with_capacity(count);
        for _ in 0..count {
            let nonce = reader.read_u64()?;
            if nonces.last().is_some_and(|&last| nonce <= last) {
                return Err(NodeError::Decode(
                    "share nonces must be strictly ascending".into(),
                ));
            }
            nonces.push(nonce);
        }
        Ok(Self {
            kind,
            epoch,
            nonces,
        })
    }
}
