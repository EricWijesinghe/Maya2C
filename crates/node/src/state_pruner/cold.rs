//! Fetching a pruned block back, from an archive nobody has to trust.
//!
//! Three checks, each against something this node already holds:
//!
//! 1. every archive section hashes to its CID ([`maya_archive::open_archive`]);
//! 2. the archive's root is the one the receipt recorded when this node wrote
//!    it and read it back;
//! 3. the decoded block's id is the id of the **header this node kept**, and
//!    its transactions match that header's `tx_root`.
//!
//! The third is what makes an untrusted archive safe. The kept header's proof
//! of work was checked when it arrived, and the header commits to the
//! transactions. Before the header carried `tx_root`, a body fetched from
//! anywhere could not have been tied to anything.

use maya_archive::{ArchiveStore, Cid, Locator, open_archive};

use crate::core::Block;
use crate::error::{NodeError, Result};
use crate::state::db::StateDB;
use crate::state_pruner::archive::archive_err;

/// Fetches pruned blocks from a set of stores.
pub struct ColdBlocks {
    stores: Vec<Box<dyn ArchiveStore>>,
}

impl ColdBlocks {
    /// A fetcher that tries `stores` in order.
    #[must_use]
    pub fn new(stores: Vec<Box<dyn ArchiveStore>>) -> Self {
        Self { stores }
    }

    /// The active-chain block at `height`, from local storage if it is still
    /// there, otherwise from the archive recorded for it.
    ///
    /// # Errors
    ///
    /// - [`NodeError::Network`] if the height is not on the active chain.
    /// - [`NodeError::Archive`] if no receipt covers it, or no store returned
    ///   a copy that verified. The last store's error is reported.
    pub fn fetch(&self, state: &StateDB, height: u64) -> Result<Block> {
        let id = state
            .canonical_id(height)?
            .ok_or_else(|| NodeError::Network(format!("no block at height {height}")))?;
        if let Some(block) = state.load_block(&id)? {
            return Ok(block);
        }
        let receipt = state.receipt_for(height)?.ok_or_else(|| {
            NodeError::Archive(format!("height {height} was pruned without an archive"))
        })?;
        let root =
            Cid::try_from(receipt.root.as_str()).map_err(|e| NodeError::Archive(e.to_string()))?;

        let mut last_error = NodeError::Archive("no configured store can read this receipt".into());
        for text in &receipt.locators {
            let locator = Locator::decode(text).map_err(archive_err)?;
            for store in self.stores.iter().filter(|s| s.kind() == locator.kind) {
                match Self::from_store(store.as_ref(), &locator, &root, height, &id) {
                    Ok(block) => return Ok(block),
                    Err(error) => last_error = error,
                }
            }
        }
        Err(last_error)
    }

    fn from_store(
        store: &dyn ArchiveStore,
        locator: &Locator,
        root: &Cid,
        height: u64,
        id: &[u8; 32],
    ) -> Result<Block> {
        let car = store.get(locator, root).map_err(archive_err)?;
        let blocks = open_archive(&car, root).map_err(archive_err)?;
        let archived = blocks
            .into_iter()
            .find(|block| block.height == height)
            .ok_or_else(|| NodeError::Archive(format!("archive {root} lacks height {height}")))?;
        let block = Block::from_bytes(&archived.bytes)?;
        if block.header.id() != *id {
            return Err(NodeError::Archive(format!(
                "archive {root} holds a different block at height {height}"
            )));
        }
        block.check_tx_root()?;
        Ok(block)
    }
}
