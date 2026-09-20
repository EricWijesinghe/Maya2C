//! The block store: every block this node has accepted, kept in the same
//! RocksDB as state.
//!
//! ## Why it exists
//!
//! Until 2026-09-12 no block was persisted anywhere. `Chain` held every block
//! in memory, and RocksDB held only the current state and the undo journals. A
//! node that had applied one block could not restart. On boot `seed_state`
//! rewrote the genesis allocations over the evolved state, the genesis root
//! check then failed, and `Chain::new` rebuilt from genesis alone.
//!
//! ## Why the same database
//!
//! A block's canonical-index entry and the tip pointer are written in the
//! **same `WriteBatch`** as the state that block produced (see
//! `StateDB::apply_canonical` and `StateDB::revert_canonical`). A crash can
//! therefore never leave state at one block and the tip at another. A second
//! database would need a two-phase commit to promise that.
//!
//! ## Layout
//!
//! | Key | Value |
//! |---|---|
//! | `blk:h:<id>` | header (144 B) ‖ height (u64 LE) ‖ cumulative work (U256 BE) |
//! | `blk:b:<id>` | the block's wire encoding |
//! | `blk:n:<height BE>` | id of the active-chain block at that height |
//! | `blk:meta` | genesis id ‖ tip id ‖ prune horizon (u64 LE) |
//!
//! Headers stay for every block, forever: 144 bytes each, carrying the
//! `state_root` and `tx_root` a pruned body is verified against. Bodies and
//! undo journals are what `state_pruner` removes.
//!
//! Every key here is under
//! [`BLOCK_STORE_PREFIX`](crate::state::commitments::BLOCK_STORE_PREFIX), which is local-only: never
//! in the state root and never carried in a snapshot.

use rocksdb::WriteBatch;

use crate::consensus::uint::U256;
use crate::core::codec::ByteReader;
use crate::core::{Block, BlockHeader, HEADER_LEN};
use crate::error::{NodeError, Result};
use crate::state::context::BlockContext;
use crate::state::db::StateDB;
use crate::state::merkle::HASH_LEN;

const HEADER_KEY: &[u8] = b"blk:h:";
const BODY_KEY: &[u8] = b"blk:b:";
const CANONICAL_KEY: &[u8] = b"blk:n:";
const META_KEY: &[u8] = b"blk:meta";

/// Length of an encoded [`StoredHeader`].
const STORED_HEADER_LEN: usize = HEADER_LEN + 8 + 32;

/// Length of an encoded [`ChainMeta`].
const META_LEN: usize = 32 + 32 + 8;

/// A header as the block store keeps it: with what the chain index needs to
/// rebuild itself on restart without reading a single body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredHeader {
    /// The header.
    pub header: BlockHeader,
    /// Distance from genesis.
    pub height: u64,
    /// Cumulative work of this block and every ancestor.
    pub total_work: U256,
}

impl StoredHeader {
    fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(STORED_HEADER_LEN);
        buf.extend_from_slice(&self.header.serialize());
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.extend_from_slice(&self.total_work.to_be_bytes());
        buf
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let header = BlockHeader::from_bytes(reader.read_slice(HEADER_LEN)?)?;
        let height = reader.read_u64()?;
        let total_work = U256::from_be_bytes(&reader.read_array::<32>()?);
        reader.finish()?;
        Ok(Self {
            header,
            height,
            total_work,
        })
    }
}

/// The three facts that locate a stored chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainMeta {
    /// Id of the genesis block this database was initialised with.
    pub genesis: [u8; HASH_LEN],
    /// Id of the active chain's tip. Moves in the same batch as state.
    pub tip: [u8; HASH_LEN],
    /// Highest height whose body and undo journal have been pruned; zero on a
    /// node that has pruned nothing.
    pub prune_horizon: u64,
}

impl ChainMeta {
    fn encode(&self) -> [u8; META_LEN] {
        let mut buf = [0u8; META_LEN];
        buf[..32].copy_from_slice(&self.genesis);
        buf[32..64].copy_from_slice(&self.tip);
        buf[64..].copy_from_slice(&self.prune_horizon.to_le_bytes());
        buf
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let meta = Self {
            genesis: reader.read_array::<32>()?,
            tip: reader.read_array::<32>()?,
            prune_horizon: reader.read_u64()?,
        };
        reader.finish()?;
        Ok(meta)
    }
}

fn keyed(prefix: &[u8], suffix: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + suffix.len());
    key.extend_from_slice(prefix);
    key.extend_from_slice(suffix);
    key
}

pub(crate) fn header_key(id: &[u8; HASH_LEN]) -> Vec<u8> {
    keyed(HEADER_KEY, id)
}

pub(crate) fn body_key(id: &[u8; HASH_LEN]) -> Vec<u8> {
    keyed(BODY_KEY, id)
}

/// Big-endian, so the canonical index iterates in height order.
pub(crate) fn canonical_key(height: u64) -> Vec<u8> {
    keyed(CANONICAL_KEY, &height.to_be_bytes())
}

impl StateDB {
    /// The stored chain's metadata, or `None` on a database no chain has been
    /// opened on yet.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure, or
    /// [`NodeError::Decode`] if the record is malformed.
    pub fn chain_meta(&self) -> Result<Option<ChainMeta>> {
        self.raw_get(META_KEY)?
            .map(|bytes| ChainMeta::decode(&bytes))
            .transpose()
    }

    pub(crate) fn put_meta(batch: &mut WriteBatch, meta: &ChainMeta) {
        batch.put(META_KEY, meta.encode());
    }

    /// The stored metadata, which a caller past initialisation requires.
    pub(crate) fn require_meta(&self) -> Result<ChainMeta> {
        self.chain_meta()?
            .ok_or_else(|| NodeError::Storage("block store has no chain metadata".to_string()))
    }

    /// Initialises the block store with `genesis` as both the first block and
    /// the tip. Called once, on a fresh database.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the write fails.
    pub fn init_chain(&self, genesis: &Block, total_work: U256) -> Result<()> {
        let id = genesis.header.id();
        let mut batch = WriteBatch::default();
        Self::put_block_record(&mut batch, genesis, 0, total_work);
        batch.put(canonical_key(0), id);
        Self::put_meta(
            &mut batch,
            &ChainMeta {
                genesis: id,
                tip: id,
                prune_horizon: 0,
            },
        );
        self.write_batch(batch)
    }

    fn put_block_record(batch: &mut WriteBatch, block: &Block, height: u64, total_work: U256) {
        let id = block.header.id();
        let stored = StoredHeader {
            header: block.header.clone(),
            height,
            total_work,
        };
        batch.put(header_key(&id), stored.encode());
        batch.put(body_key(&id), block.to_bytes());
    }

    /// Stores a block the chain has accepted into its index, whether or not it
    /// is on the active chain. Moves neither the tip nor state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the write fails.
    pub fn store_block(&self, block: &Block, height: u64, total_work: U256) -> Result<()> {
        let mut batch = WriteBatch::default();
        Self::put_block_record(&mut batch, block, height, total_work);
        self.write_batch(batch)
    }

    /// Stores a header without its body: a header validated ahead of the
    /// body, as a pruned node's bootstrap does.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the write fails.
    pub fn store_header(&self, stored: &StoredHeader) -> Result<()> {
        self.raw_put(&header_key(&stored.header.id()), &stored.encode())
    }

    /// Forgets a block that was stored and then refused: its header and body.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the write fails.
    pub fn delete_block(&self, id: &[u8; HASH_LEN]) -> Result<()> {
        let mut batch = WriteBatch::default();
        batch.delete(header_key(id));
        batch.delete(body_key(id));
        self.write_batch(batch)
    }

    /// Every stored header, in no particular order.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on an iteration failure, or
    /// [`NodeError::Decode`] if a record is malformed.
    pub fn stored_headers(&self) -> Result<Vec<StoredHeader>> {
        self.scan_prefix(HEADER_KEY)?
            .iter()
            .map(|(_, value)| StoredHeader::decode(value))
            .collect()
    }

    /// One stored header.
    ///
    /// # Errors
    ///
    /// As [`StateDB::stored_headers`].
    pub fn stored_header(&self, id: &[u8; HASH_LEN]) -> Result<Option<StoredHeader>> {
        self.raw_get(&header_key(id))?
            .map(|bytes| StoredHeader::decode(&bytes))
            .transpose()
    }

    /// Whether the body of `id` is held locally.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn has_body(&self, id: &[u8; HASH_LEN]) -> Result<bool> {
        Ok(self.raw_get(&body_key(id))?.is_some())
    }

    /// The raw wire encoding of a locally held block.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn block_bytes(&self, id: &[u8; HASH_LEN]) -> Result<Option<Vec<u8>>> {
        self.raw_get(&body_key(id))
    }

    /// A locally held block, or `None` if its body was pruned or never held.
    ///
    /// The decoded block's id is checked against the key it was stored under.
    /// A mismatch means the database was damaged or written by something else,
    /// and returning the block would put the wrong transactions behind a
    /// header.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure or an id mismatch, or
    /// [`NodeError::Decode`] if the stored bytes are malformed.
    pub fn load_block(&self, id: &[u8; HASH_LEN]) -> Result<Option<Block>> {
        let Some(bytes) = self.block_bytes(id)? else {
            return Ok(None);
        };
        let block = Block::from_bytes(&bytes)?;
        if block.header.id() != *id {
            return Err(NodeError::Storage(format!(
                "block stored under {} decodes to a different id",
                hex::encode(id)
            )));
        }
        Ok(Some(block))
    }

    /// The id of the active-chain block at `height`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure or a malformed entry.
    pub fn canonical_id(&self, height: u64) -> Result<Option<[u8; HASH_LEN]>> {
        self.raw_get(&canonical_key(height))?
            .map(|bytes| {
                <[u8; HASH_LEN]>::try_from(bytes.as_slice()).map_err(|_| {
                    NodeError::Storage(format!("malformed canonical entry at height {height}"))
                })
            })
            .transpose()
    }

    /// Height of the stored tip.
    ///
    /// Two point lookups rather than a cached field, because a cached height
    /// is a second copy of the tip that a crash mid-reorg could leave
    /// disagreeing with the one in the batch.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if the store has no metadata or no
    /// header for the tip it names.
    pub fn tip_height(&self) -> Result<u64> {
        let tip = self.require_meta()?.tip;
        self.stored_header(&tip)?
            .map(|stored| stored.height)
            .ok_or_else(|| NodeError::Storage(format!("no header for tip {}", hex::encode(tip))))
    }

    /// Applies `block` as the new tip: the chain's checked, journaled apply,
    /// with the canonical index and the tip pointer in the same batch.
    ///
    /// # Errors
    ///
    /// As [`StateDB::apply_block_journaled`].
    pub(crate) fn apply_canonical(
        &self,
        block: &Block,
        id: &[u8; HASH_LEN],
        height: u64,
        context: BlockContext,
    ) -> Result<[u8; HASH_LEN]> {
        let meta = self.require_meta()?;
        self.apply_journaled_with(block, id, context, |batch| {
            batch.put(canonical_key(height), id);
            Self::put_meta(batch, &ChainMeta { tip: *id, ..meta });
        })
    }

    /// Reverts the tip `id` at `height`, making `parent` the tip, with the
    /// canonical index and the tip pointer in the same batch as the revert.
    ///
    /// # Errors
    ///
    /// As [`StateDB::revert_block`].
    pub(crate) fn revert_canonical(
        &self,
        id: &[u8; HASH_LEN],
        height: u64,
        parent: &[u8; HASH_LEN],
    ) -> Result<()> {
        let meta = self.require_meta()?;
        self.revert_block_with(id, |batch| {
            batch.delete(canonical_key(height));
            Self::put_meta(
                batch,
                &ChainMeta {
                    tip: *parent,
                    ..meta
                },
            );
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::state::commitments::BLOCK_STORE_PREFIX;

    #[test]
    fn every_block_store_key_is_under_the_local_only_prefix() {
        // The snapshot and root-coverage rules treat `blk:` as local. A key
        // here outside it would be read as state.
        for key in [
            header_key(&[1; 32]),
            body_key(&[1; 32]),
            canonical_key(7),
            META_KEY.to_vec(),
        ] {
            assert!(key.starts_with(BLOCK_STORE_PREFIX), "{key:?}");
        }
    }

    #[test]
    fn metadata_round_trips() {
        let meta = ChainMeta {
            genesis: [1; 32],
            tip: [2; 32],
            prune_horizon: 30_000,
        };
        assert_eq!(ChainMeta::decode(&meta.encode()), Ok(meta));
        assert!(ChainMeta::decode(&meta.encode()[..META_LEN - 1]).is_err());
    }

    #[test]
    fn canonical_keys_sort_by_height() {
        // Big-endian: a prefix scan visits heights in order, which the pruner
        // relies on to walk a range.
        assert!(canonical_key(255) < canonical_key(256));
        assert!(canonical_key(1) < canonical_key(u64::MAX));
    }
}
