//! The uploader: archive a batch, read every copy back, and only then prune.

use std::ops::RangeInclusive;
use std::sync::Mutex;

use maya_archive::{ArchiveStore, ArchivedBlock, build_archive, open_archive};

use crate::consensus::Chain;
use crate::error::{NodeError, Result};
use crate::state_pruner::{ArchivePolicy, ArchiveReceipt, PruneConfig};

/// Turns an archive error into a node error.
pub(crate) fn archive_err(error: maya_archive::ArchiveError) -> NodeError {
    NodeError::Archive(error.to_string())
}

/// Writes batches to a set of stores.
pub struct Archiver {
    chain_id: String,
    stores: Vec<Box<dyn ArchiveStore>>,
}

impl Archiver {
    /// An archiver writing to every one of `stores`.
    #[must_use]
    pub fn new(chain_id: impl Into<String>, stores: Vec<Box<dyn ArchiveStore>>) -> Self {
        Self {
            chain_id: chain_id.into(),
            stores,
        }
    }

    /// Archives `blocks` to every store, reads each copy back, and returns a
    /// receipt only if every copy verified and matched.
    ///
    /// All stores, not the first that succeeds: a receipt lists every copy the
    /// cold fetcher may try. A store that fails makes the whole round fail,
    /// and nothing is pruned. The operator configured that store, and silently
    /// running with fewer copies than configured is how the last one goes
    /// missing.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Archive`] if there is no store, any store fails,
    /// or any copy reads back different from what was written.
    pub fn archive(&self, blocks: &[ArchivedBlock]) -> Result<ArchiveReceipt> {
        if self.stores.is_empty() {
            return Err(NodeError::Archive(
                "no archive store configured, and the policy requires one".to_string(),
            ));
        }
        let archive = build_archive(&self.chain_id, blocks).map_err(archive_err)?;
        let mut locators = Vec::with_capacity(self.stores.len());
        for store in &self.stores {
            let locator = store.put(&archive).map_err(archive_err)?;
            let copy = store.get(&locator, &archive.root).map_err(archive_err)?;
            let read_back = open_archive(&copy, &archive.root).map_err(archive_err)?;
            if read_back != blocks {
                return Err(NodeError::Archive(format!(
                    "{} store returned a different batch for {}",
                    store.kind(),
                    archive.root
                )));
            }
            locators.push(locator.encode());
        }
        Ok(ArchiveReceipt {
            first_height: archive.manifest.first_height,
            count: blocks.len() as u64,
            root: archive.root.to_string(),
            locators,
        })
    }
}

fn lock(chain: &Mutex<Chain>) -> std::sync::MutexGuard<'_, Chain> {
    chain
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One pruning round: pick the next batch deep enough to prune, archive it if
/// the policy requires, and prune it.
///
/// The chain lock is released while archiving, which can take minutes against
/// a remote store, and taken again to prune. [`Chain::prune`] then checks the
/// batch is still the active chain's, so a reorg in between turns the round
/// into a no-op rather than deleting the wrong bodies.
///
/// Returns the pruned range, or `None` when nothing is deep enough yet.
///
/// # Errors
///
/// Any archive or storage failure. Nothing is pruned when one occurs.
pub fn prune_round(
    chain: &Mutex<Chain>,
    config: &PruneConfig,
    archiver: Option<&Archiver>,
) -> Result<Option<RangeInclusive<u64>>> {
    let (range, blocks) = {
        let chain = lock(chain);
        let Some(range) = chain.prunable(config) else {
            return Ok(None);
        };
        let blocks = chain.canonical_batch(&range)?;
        (range, blocks)
    };

    let receipt = match (config.archive, archiver) {
        (ArchivePolicy::None, _) => None,
        (ArchivePolicy::Required, Some(archiver)) => Some(archiver.archive(&blocks)?),
        (ArchivePolicy::Required, None) => {
            return Err(NodeError::Archive(
                "pruning requires an archive and none is configured".to_string(),
            ));
        }
    };

    let ids: Vec<[u8; 32]> = blocks.iter().map(|block| block.id).collect();
    lock(chain).prune(range.clone(), &ids, receipt.as_ref())?;
    Ok(Some(range))
}
