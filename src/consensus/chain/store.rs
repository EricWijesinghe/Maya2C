//! The persistent half of [`Chain`]: opening a stored chain, importing headers
//! ahead of their bodies, adopting a snapshot, and pruning.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use maya_archive::ArchivedBlock;
use rocksdb::WriteBatch;

use super::{BlockId, BlockRecord, Chain, ChainConfig};
use crate::consensus::difficulty::work_from_target;
use crate::core::{Block, BlockHeader};
use crate::crypto::pow::meets_target;
use crate::error::{NodeError, Result};
use crate::state::StateDB;
use crate::state::blocks::{ChainMeta, StoredHeader, body_key, canonical_key, header_key};
use crate::state::db::undo_key;
use crate::state_pruner::{ArchiveReceipt, PruneConfig};

impl Chain {
    /// Opens the chain stored in `state`, or starts one at `genesis` if the
    /// database holds none.
    ///
    /// On a fresh database `genesis` is stored as the first block and the tip.
    /// Its transactions are not executed: genesis defines the starting state
    /// rather than transitioning into it, so seed any allocations into `state`
    /// before the first open.
    ///
    /// On an existing database the index is rebuilt from the stored headers
    /// without reading a body, and three things are checked:
    /// - the stored genesis is the one configured;
    /// - the state is the post-state of the stored tip;
    /// - no stored branch has more work than the tip.
    ///
    /// The last one recovers from a crash in the middle of a reorg. Every
    /// revert and apply moves the tip in the same batch as state, so a crash
    /// leaves a consistent tip on the losing branch, and opening finishes the
    /// switch.
    ///
    /// # Errors
    ///
    /// - [`NodeError::GenesisMismatch`] if the database was started with
    ///   another genesis.
    /// - [`NodeError::StateRootMismatch`] if state is not the tip's post-state.
    /// - [`NodeError::Storage`] or [`NodeError::Decode`] for a damaged store.
    pub fn open(state: Arc<StateDB>, genesis: Block, config: ChainConfig) -> Result<Self> {
        let genesis_id = genesis.header.id();
        let meta = match state.chain_meta()? {
            Some(meta) => meta,
            None => {
                state.init_chain(
                    &genesis,
                    work_from_target(&genesis.header.difficulty_target),
                )?;
                state.require_meta()?
            }
        };
        if meta.genesis != genesis_id {
            return Err(NodeError::GenesisMismatch {
                stored: hex::encode(meta.genesis),
                configured: hex::encode(genesis_id),
            });
        }

        let records = Self::load_records(&state)?;
        if !records.contains_key(&meta.tip) {
            return Err(NodeError::Storage(format!(
                "stored tip {} has no header",
                hex::encode(meta.tip)
            )));
        }

        let mut chain = Self {
            dag: Arc::new(crate::crypto::dag::registry::CacheRegistry::new(config.dag)),
            state,
            config,
            records,
            tip: meta.tip,
            genesis: genesis_id,
            prune_horizon: meta.prune_horizon,
        };
        chain.check_tip_state()?;
        chain.adopt_heaviest();
        Ok(chain)
    }

    fn load_records(state: &StateDB) -> Result<HashMap<BlockId, BlockRecord>> {
        Ok(state
            .stored_headers()?
            .into_iter()
            .map(|stored| {
                let parent = (stored.height > 0).then_some(stored.header.prev_hash);
                (
                    stored.header.id(),
                    BlockRecord {
                        header: stored.header,
                        height: stored.height,
                        total_work: stored.total_work,
                        parent,
                    },
                )
            })
            .collect())
    }

    /// Requires committed state to be the stored tip's post-state.
    ///
    /// Skipped at genesis, whose header root describes the configured
    /// allocations, which a test or a pruned bootstrap may not have seeded.
    fn check_tip_state(&self) -> Result<()> {
        if self.tip == self.genesis {
            return Ok(());
        }
        let expected = self.require(&self.tip)?.header.state_root;
        let actual = self.state.state_root()?;
        if actual != expected {
            return Err(NodeError::StateRootMismatch {
                expected: hex::encode(expected),
                actual: hex::encode(actual),
            });
        }
        Ok(())
    }

    /// Switches to the stored branch with the most work, if it beats the tip
    /// and its body is held.
    ///
    /// Best effort, on purpose. A failed switch restores the original branch,
    /// and the node then runs on the tip it has. That is what it would do had
    /// the heavier branch simply not arrived yet.
    fn adopt_heaviest(&mut self) {
        let tip_work = self.total_work();
        let best = self
            .records
            .iter()
            .filter(|(_, record)| record.total_work > tip_work)
            .max_by_key(|(_, record)| record.total_work)
            .map(|(id, _)| *id);
        if let Some(best) = best
            && self.state.has_body(&best).unwrap_or(false)
        {
            let _ = self.reorganize(&best);
        }
    }

    /// Validates `headers` as a linked extension of the index, without their
    /// bodies, and records them. The tip does not move.
    ///
    /// The same checks a block gets before its body matters: a known parent,
    /// the exact target the retarget rule produces, and a proof of work that
    /// meets it. What this cannot check is state, which is what the snapshot
    /// root and the bodies above it are for.
    ///
    /// Returns each header's id, in order.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] for an unknown parent, a wrong target, or
    /// insufficient work, having recorded the headers before it.
    pub fn import_headers(&mut self, headers: &[BlockHeader]) -> Result<Vec<BlockId>> {
        let mut ids = Vec::with_capacity(headers.len());
        for header in headers {
            let parent_id = header.prev_hash;
            let parent = self.require(&parent_id)?;
            let height = parent.height + 1;
            let parent_work = parent.total_work;

            let expected = self.next_target(&parent_id)?;
            if header.difficulty_target != expected {
                return Err(NodeError::Network(format!(
                    "wrong difficulty target on header at height {height}"
                )));
            }
            if self.config.verify_pow
                && !meets_target(
                    &header.pow_hash_at(height, &self.dag)?,
                    &header.difficulty_target,
                )
            {
                return Err(NodeError::Network(format!(
                    "insufficient proof of work on header at height {height}"
                )));
            }

            let total_work =
                parent_work.saturating_add(work_from_target(&header.difficulty_target));
            let stored = StoredHeader {
                header: header.clone(),
                height,
                total_work,
            };
            self.state.store_header(&stored)?;
            let id = header.id();
            self.records.insert(
                id,
                BlockRecord {
                    header: header.clone(),
                    height,
                    total_work,
                    parent: Some(parent_id),
                },
            );
            ids.push(id);
        }
        Ok(ids)
    }

    /// Makes the last of `path` the tip, and the prune horizon, of a chain
    /// whose state was imported from a snapshot taken there.
    ///
    /// `path` is the active chain from genesis to the snapshot block. Every
    /// entry must already be an imported header. The canonical index, the tip
    /// and the horizon are written in one batch.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if `path` is empty or names a header the
    /// index does not hold, and [`NodeError::StateRootMismatch`] if state is
    /// not that block's post-state.
    pub fn adopt_snapshot(&mut self, path: &[BlockId]) -> Result<()> {
        let (tip, _) = path
            .split_last()
            .ok_or_else(|| NodeError::Network("empty snapshot path".to_string()))?;
        let height = self.require(tip)?.height;
        let mut batch = WriteBatch::default();
        for (index, id) in path.iter().enumerate() {
            self.require(id)?;
            batch.put(canonical_key(index as u64), id);
        }
        let meta = ChainMeta {
            genesis: self.genesis,
            tip: *tip,
            prune_horizon: height,
        };
        StateDB::put_meta(&mut batch, &meta);

        let previous = (self.tip, self.prune_horizon);
        self.tip = *tip;
        self.prune_horizon = height;
        if let Err(error) = self.check_tip_state() {
            (self.tip, self.prune_horizon) = previous;
            return Err(error);
        }
        self.state.write_batch(batch)
    }

    /// Highest height whose body has been pruned; zero if none has.
    #[must_use]
    pub fn prune_horizon(&self) -> u64 {
        self.prune_horizon
    }

    /// The next batch of active-chain heights deep enough to prune under
    /// `config`, or `None` if nothing is.
    ///
    /// Genesis is never pruned. Its body is empty and it is where every
    /// header walk ends.
    #[must_use]
    pub fn prunable(&self, config: &PruneConfig) -> Option<RangeInclusive<u64>> {
        let first = self.prune_horizon + 1;
        let deepest = self.height().checked_sub(config.depth)?;
        let last = deepest.min(first.saturating_add(config.batch.max(1) - 1));
        (last >= first).then_some(first..=last)
    }

    /// The active-chain blocks at `range`, as wire bytes ready to archive.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] if a height has no canonical entry or no
    /// body.
    pub fn canonical_batch(&self, range: &RangeInclusive<u64>) -> Result<Vec<ArchivedBlock>> {
        range
            .clone()
            .map(|height| {
                let id = self.state.canonical_id(height)?.ok_or_else(|| {
                    NodeError::Storage(format!("no canonical block at height {height}"))
                })?;
                let bytes = self.state.block_bytes(&id)?.ok_or_else(|| {
                    NodeError::Storage(format!("no body held at height {height}"))
                })?;
                Ok(ArchivedBlock { height, id, bytes })
            })
            .collect()
    }

    /// Drops the bodies and undo journals at `range`, records `receipt`, and
    /// raises the horizon, in one batch.
    ///
    /// `ids` are the blocks that were archived. If the active chain no longer
    /// holds exactly those blocks at those heights (a reorg landed while the
    /// batch was being archived), nothing is deleted. Side branches that fork
    /// at or below the new horizon are dropped whole, since they can never
    /// win without a refused reorg.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the range is not the next one above
    /// the horizon, is not deep enough to be final, or no longer matches
    /// `ids`. Returns [`NodeError::Storage`] if the batch fails.
    pub fn prune(
        &mut self,
        range: RangeInclusive<u64>,
        ids: &[BlockId],
        receipt: Option<&ArchiveReceipt>,
    ) -> Result<()> {
        if *range.start() != self.prune_horizon + 1
            || ids.len() as u64 != range.clone().count() as u64
        {
            return Err(NodeError::Network(format!(
                "prune range {range:?} does not follow horizon {}",
                self.prune_horizon
            )));
        }
        let mut batch = WriteBatch::default();
        for (height, expected) in range.clone().zip(ids) {
            if self.state.canonical_id(height)? != Some(*expected) {
                return Err(NodeError::Network(format!(
                    "the active chain moved at height {height} while it was being archived"
                )));
            }
            batch.delete(body_key(expected));
            batch.delete(undo_key(expected));
        }
        if let Some(receipt) = receipt {
            batch.put(receipt.key(), receipt.encode());
        }

        let horizon = *range.end();
        let dropped = self.stale_branches(horizon)?;
        for id in &dropped {
            batch.delete(header_key(id));
            batch.delete(body_key(id));
        }
        let meta = ChainMeta {
            genesis: self.genesis,
            tip: self.tip,
            prune_horizon: horizon,
        };
        StateDB::put_meta(&mut batch, &meta);
        self.state.write_batch(batch)?;

        for id in &dropped {
            self.records.remove(id);
        }
        self.prune_horizon = horizon;
        Ok(())
    }

    /// Off-chain records that fork at or below `horizon`, with their
    /// descendants.
    fn stale_branches(&self, horizon: u64) -> Result<Vec<BlockId>> {
        let mut stale = Vec::new();
        for (id, record) in &self.records {
            if self.state.canonical_id(record.height)? == Some(*id) {
                continue;
            }
            // Walk down to the active chain. Where the branch meets it is its
            // fork point.
            let mut cursor = record;
            let fork_height = loop {
                let Some(parent_id) = cursor.parent else {
                    break 0;
                };
                let Some(parent) = self.records.get(&parent_id) else {
                    break 0;
                };
                if self.state.canonical_id(parent.height)? == Some(parent_id) {
                    break parent.height;
                }
                cursor = parent;
            };
            if fork_height < horizon {
                stale.push(*id);
            }
        }
        Ok(stale)
    }
}
