//! Historical pruning: dropping old block bodies and undo journals, after a
//! verified copy of them exists somewhere else.
//!
//! # What "final" means here
//!
//! This chain is proof of work. Nothing is ever final, so this module does not
//! pretend it is. A **pruned node** instead adopts a local policy: a block at
//! least [`PRUNE_DEPTH`] below the tip on the active chain will not be
//! reorganised away, and such a node refuses any reorg reaching below its
//! horizon ([`crate::error::NodeError::BelowPruneHorizon`]). An archive node
//! prunes nothing, has no horizon, and is unaffected. Pruning is opt-in and
//! off by default.
//!
//! [`PRUNE_DEPTH`] is one DAG epoch, 30,000 blocks, about 5.2 days at 15-second
//! blocks. That is far deeper than any honest reorg on a working network.
//!
//! # What is kept
//!
//! Every header, forever: 144 bytes each, about 4.3 MB per 30,000 blocks.
//! A header carries `tx_root` and `state_root`, which is all a pruned body is
//! ever verified against. Those are the "light Merkle roots" left where the
//! blocks were. Bodies and undo journals below the horizon are deleted.
//!
//! # Ordering: archive, verify, then delete
//!
//! [`archive::Archiver::archive`] writes the batch to every configured store
//! and **reads each copy back and verifies it** before returning a receipt.
//! [`crate::consensus::Chain::prune`] is only ever called with that receipt.
//! So a body is never deleted before a checked copy of it exists. Pruning
//! without any archive ([`ArchivePolicy::None`]) is possible, as it is for a
//! Bitcoin pruned node, but it has to be chosen explicitly.

pub mod archive;
pub mod cold;
pub mod snapshot;

use crate::crypto::dag::EPOCH_LENGTH;
use crate::error::{NodeError, Result};
use crate::state::db::StateDB;

/// Default depth below the tip before a body may be pruned: one DAG epoch.
pub const PRUNE_DEPTH: u64 = EPOCH_LENGTH;

/// Default number of blocks archived and pruned together.
pub const PRUNE_BATCH: u64 = 1_000;

/// Whether pruning requires an archive first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArchivePolicy {
    /// Every pruned batch must first be archived and read back. The default
    /// whenever pruning is on.
    Required,
    /// Prune without keeping a copy anywhere. History below the horizon is
    /// then only available from other nodes' archives.
    None,
}

/// A pruned node's policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PruneConfig {
    /// Blocks at least this far below the tip may lose their bodies.
    pub depth: u64,
    /// How many blocks to archive and prune at once.
    pub batch: u64,
    /// Whether a verified archive is required first.
    pub archive: ArchivePolicy,
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            depth: PRUNE_DEPTH,
            batch: PRUNE_BATCH,
            archive: ArchivePolicy::Required,
        }
    }
}

const RECEIPT_KEY: &[u8] = b"blk:arch:";

/// Proof that a batch of pruned blocks was archived, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchiveReceipt {
    /// Height of the first block in the batch.
    pub first_height: u64,
    /// Number of blocks in the batch.
    pub count: u64,
    /// CID of the archive's manifest: what every copy is verified against.
    pub root: String,
    /// Where copies were written and read back from, as `kind:reference`.
    pub locators: Vec<String>,
}

fn put_text(buf: &mut Vec<u8>, text: &str) {
    buf.extend_from_slice(&(text.len() as u64).to_le_bytes());
    buf.extend_from_slice(text.as_bytes());
}

fn read_text(reader: &mut crate::core::codec::ByteReader<'_>) -> Result<String> {
    let len = reader.read_collection_len(1)?;
    String::from_utf8(reader.read_slice(len)?.to_vec())
        .map_err(|_| NodeError::Decode("receipt text is not UTF-8".to_string()))
}

impl ArchiveReceipt {
    /// Whether `height` falls in this batch.
    #[must_use]
    pub fn covers(&self, height: u64) -> bool {
        height >= self.first_height && height - self.first_height < self.count
    }

    /// Encodes the receipt for the block store.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&self.first_height.to_le_bytes());
        buf.extend_from_slice(&self.count.to_le_bytes());
        put_text(&mut buf, &self.root);
        buf.extend_from_slice(&(self.locators.len() as u64).to_le_bytes());
        for locator in &self.locators {
            put_text(&mut buf, locator);
        }
        buf
    }

    /// Decodes a stored receipt.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is malformed.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = crate::core::codec::ByteReader::new(bytes);
        let first_height = reader.read_u64()?;
        let count = reader.read_u64()?;
        let root = read_text(&mut reader)?;
        let locator_count = reader.read_collection_len(8)?;
        let mut locators = Vec::with_capacity(locator_count);
        for _ in 0..locator_count {
            locators.push(read_text(&mut reader)?);
        }
        reader.finish()?;
        Ok(Self {
            first_height,
            count,
            root,
            locators,
        })
    }

    pub(crate) fn key(&self) -> Vec<u8> {
        let mut key = RECEIPT_KEY.to_vec();
        key.extend_from_slice(&self.first_height.to_be_bytes());
        key
    }
}

impl StateDB {
    /// The receipt of the archive holding `height`, if it was archived.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure, or
    /// [`NodeError::Decode`] if a receipt is malformed.
    pub fn receipt_for(&self, height: u64) -> Result<Option<ArchiveReceipt>> {
        // One receipt per batch, so a scan is a few thousand records at most
        // on a chain millions of blocks long.
        for (_, value) in self.scan_prefix(RECEIPT_KEY)? {
            let receipt = ArchiveReceipt::decode(&value)?;
            if receipt.covers(height) {
                return Ok(Some(receipt));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_receipt_round_trips_and_knows_its_range() {
        let receipt = ArchiveReceipt {
            first_height: 1_000,
            count: 500,
            root: "bafyroot".into(),
            locators: vec!["local:x.car.zst".into(), "ipfs:bafyroot".into()],
        };
        assert_eq!(
            ArchiveReceipt::decode(&receipt.encode()),
            Ok(receipt.clone())
        );
        assert!(receipt.covers(1_000) && receipt.covers(1_499));
        assert!(!receipt.covers(999) && !receipt.covers(1_500));
    }

    #[test]
    fn the_receipt_key_is_local_only() {
        let receipt = ArchiveReceipt {
            first_height: 1,
            count: 1,
            root: String::new(),
            locators: Vec::new(),
        };
        assert!(
            receipt
                .key()
                .starts_with(crate::state::commitments::BLOCK_STORE_PREFIX)
        );
    }

    #[test]
    fn the_default_depth_is_one_dag_epoch() {
        assert_eq!(PruneConfig::default().depth, 30_000);
        assert_eq!(PruneConfig::default().archive, ArchivePolicy::Required);
    }
}
