//! Block index, fork choice, and chain reorganization.
//!
//! ## Fork choice
//!
//! The active chain is the one with the greatest **cumulative work**, never the
//! greatest height. A long branch of easy blocks represents less expended
//! hashpower than a short branch of hard ones, and height cannot distinguish
//! them.
//!
//! Ties are broken by keeping the incumbent tip. Switching on equal work would
//! let a peer flip the active chain back and forth at no cost.
//!
//! ## Reorganization
//!
//! Switching branches is: walk both tips back to their common ancestor, revert
//! the abandoned blocks newest-first through the undo journal, then apply the
//! new branch oldest-first.
//!
//! If applying the new branch fails partway — an invalid transaction in a block
//! a peer sent — the chain must not be left straddling two branches. Everything
//! applied so far is rolled back and the original branch is restored before the
//! error is returned, so a failed reorg is a no-op rather than corruption.

use std::collections::HashMap;
use std::sync::Arc;

use crate::consensus::difficulty::{
    EXPECTED_TIMESPAN, RETARGET_INTERVAL, default_pow_limit, is_retarget_height, retarget,
    unlimited_pow_limit, work_from_target,
};
use crate::consensus::uint::U256;
use crate::core::{Block, BlockHeader};
use crate::crypto::dag::registry::{CacheRegistry, DagConfig};
use crate::crypto::pow::meets_target;
use crate::error::{NodeError, Result};
use crate::state::{BlockContext, StateDB};

/// Content-addressed block identifier. See [`BlockHeader::id`].
pub type BlockId = [u8; 32];

/// Network parameters and validation switches.
#[derive(Clone, Copy, Debug)]
pub struct ChainConfig {
    /// Whether to verify each block's proof of work.
    pub verify_pow: bool,
    /// Easiest permitted target. Retargeting never eases past this.
    pub pow_limit: [u8; 32],
    /// Which proof-of-work rule applies at which height, and at what sizes.
    ///
    /// Defaults to [`DagConfig::MAINNET`]: ArgonBlake below
    /// [`DAG_ACTIVATION_HEIGHT`], the 4 GiB DAG at and above it.
    ///
    /// [`DAG_ACTIVATION_HEIGHT`]: crate::crypto::dag::DAG_ACTIVATION_HEIGHT
    pub dag: DagConfig,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            verify_pow: true,
            pow_limit: default_pow_limit(),
            dag: DagConfig::MAINNET,
        }
    }
}

impl ChainConfig {
    /// Configuration for a private or test network: no proof-of-work
    /// verification and no difficulty floor.
    ///
    /// Each verification costs a full 32 MiB Argon2id pass, so a test that
    /// builds a multi-block fork would spend minutes mining blocks whose PoW is
    /// irrelevant to the behaviour under test. Never use this on a node that
    /// accepts blocks from peers.
    #[must_use]
    pub fn without_pow_verification() -> Self {
        Self {
            verify_pow: false,
            pow_limit: unlimited_pow_limit(),
            dag: DagConfig::MAINNET,
        }
    }

    /// Configuration that verifies proof of work against a custom floor.
    ///
    /// Used by the miner to run a low-difficulty chain whose genesis target
    /// would be rejected by the mainnet floor.
    #[must_use]
    pub fn with_pow_limit(pow_limit: [u8; 32]) -> Self {
        Self {
            verify_pow: true,
            pow_limit,
            dag: DagConfig::MAINNET,
        }
    }

    /// The same configuration with a different proof-of-work schedule.
    ///
    /// Used by tests that need the DAG rule to apply at a height they can
    /// actually reach, at sizes a CI runner can actually allocate.
    #[must_use]
    pub fn with_dag(self, dag: DagConfig) -> Self {
        Self { dag, ..self }
    }
}

/// A block plus the index metadata derived from it.
#[derive(Clone, Debug)]
pub struct BlockRecord {
    /// The block itself.
    pub block: Block,
    /// Distance from genesis.
    pub height: u64,
    /// Cumulative work of this block and all its ancestors.
    pub total_work: U256,
    /// Parent block id, or `None` for genesis.
    pub parent: Option<BlockId>,
}

/// What happened when a block was inserted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InsertOutcome {
    /// The block extended the active chain.
    Extended {
        /// The new tip.
        tip: BlockId,
    },
    /// The active chain switched branches.
    Reorganized {
        /// The new tip.
        tip: BlockId,
        /// Blocks rolled back, newest first.
        reverted: Vec<BlockId>,
        /// Blocks applied, oldest first.
        applied: Vec<BlockId>,
    },
    /// Stored on a branch with no more work than the active chain.
    SideBranch {
        /// Id of the stored block.
        id: BlockId,
    },
    /// Already known; nothing changed.
    Duplicate {
        /// Id of the block already held.
        id: BlockId,
    },
}

/// Block index over a [`StateDB`], maintaining the active chain.
pub struct Chain {
    state: Arc<StateDB>,
    config: ChainConfig,
    records: HashMap<BlockId, BlockRecord>,
    tip: BlockId,
    genesis: BlockId,
    /// Epoch caches for DAG verification. Shared, because the miner reads the
    /// same caches the validator does — and must, or it would mine against a
    /// dataset the chain does not recognise.
    dag: Arc<CacheRegistry>,
}

impl Chain {
    /// Creates a chain rooted at `genesis`.
    ///
    /// The genesis block's transactions are not executed: it defines the
    /// starting state rather than transitioning into it, so seed any premined
    /// balances through [`StateDB::put_account`] before calling this.
    #[must_use]
    pub fn new(state: Arc<StateDB>, genesis: Block, config: ChainConfig) -> Self {
        let id = genesis.header.id();
        let total_work = work_from_target(&genesis.header.difficulty_target);

        let mut records = HashMap::new();
        records.insert(
            id,
            BlockRecord {
                block: genesis,
                height: 0,
                total_work,
                parent: None,
            },
        );

        Self {
            state,
            config,
            records,
            tip: id,
            genesis: id,
            // Empty until a block at or above the activation height arrives, so
            // a pre-fork chain never allocates a cache it will not use.
            dag: Arc::new(CacheRegistry::new(config.dag)),
        }
    }

    /// The epoch caches this chain validates against.
    ///
    /// Handed to a miner so it searches against the same caches the chain
    /// checks with, and so it can call `prepare` ahead of an epoch boundary.
    #[must_use]
    pub fn dag(&self) -> &Arc<CacheRegistry> {
        &self.dag
    }

    /// The active chain tip.
    #[must_use]
    pub fn tip(&self) -> BlockId {
        self.tip
    }

    /// Height of the active chain tip.
    #[must_use]
    pub fn height(&self) -> u64 {
        self.records
            .get(&self.tip)
            .map_or(0, |record| record.height)
    }

    /// Cumulative work of the active chain.
    #[must_use]
    pub fn total_work(&self) -> U256 {
        self.records
            .get(&self.tip)
            .map_or(U256::ZERO, |record| record.total_work)
    }

    /// The genesis block id.
    #[must_use]
    pub fn genesis(&self) -> BlockId {
        self.genesis
    }

    /// Looks up an indexed block.
    #[must_use]
    pub fn get(&self, id: &BlockId) -> Option<&BlockRecord> {
        self.records.get(id)
    }

    /// Whether a block is known.
    #[must_use]
    pub fn contains(&self, id: &BlockId) -> bool {
        self.records.contains_key(id)
    }

    /// Shared state handle.
    #[must_use]
    pub fn state(&self) -> &Arc<StateDB> {
        &self.state
    }

    /// The target a child of `parent_id` must satisfy.
    ///
    /// Retargets on interval boundaries using the elapsed time across the
    /// window; otherwise inherits the parent's target.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if `parent_id` is unknown or the chain
    /// index is inconsistent.
    pub fn next_target(&self, parent_id: &BlockId) -> Result<[u8; 32]> {
        let parent = self.require(parent_id)?;
        let child_height = parent.height + 1;

        // The fork block does not inherit. The rule changing means the cost of
        // a hash changes by orders of magnitude, and a target calibrated for
        // the old rule would take many retarget windows to unwind — see
        // `DagConfig::activation_target`.
        if child_height == self.config.dag.activation_height {
            let pinned = self.config.dag.activation_target;
            // Never easier than the network's floor, which the pin is not
            // allowed to override any more than a retarget is.
            return Ok(if pinned > self.config.pow_limit {
                self.config.pow_limit
            } else {
                pinned
            });
        }

        if !is_retarget_height(child_height) {
            return Ok(parent.block.header.difficulty_target);
        }

        // Walk back RETARGET_INTERVAL blocks to find the window's first block.
        let mut cursor = parent;
        for _ in 0..RETARGET_INTERVAL.saturating_sub(1) {
            match cursor.parent.and_then(|id| self.records.get(&id)) {
                Some(previous) => cursor = previous,
                // Window extends past genesis: not enough history to retarget.
                None => return Ok(parent.block.header.difficulty_target),
            }
        }

        // Saturating: a parent timestamp behind the window start would
        // otherwise underflow. Zero clamps up to the minimum band below.
        let actual_timespan = parent
            .block
            .header
            .timestamp
            .saturating_sub(cursor.block.header.timestamp);

        Ok(retarget(
            &parent.block.header.difficulty_target,
            actual_timespan,
            &self.config.pow_limit,
        ))
    }

    fn require(&self, id: &BlockId) -> Result<&BlockRecord> {
        self.records
            .get(id)
            .ok_or_else(|| NodeError::Network(format!("unknown block {}", hex::encode(id))))
    }

    /// Validates and inserts a block, reorganizing if it wins fork choice.
    ///
    /// # Errors
    ///
    /// Returns an error if the parent is unknown, the declared difficulty
    /// target is wrong, the proof of work is insufficient, or applying the
    /// block's transactions fails.
    pub fn insert_block(&mut self, block: Block) -> Result<InsertOutcome> {
        let id = block.header.id();
        if self.records.contains_key(&id) {
            return Ok(InsertOutcome::Duplicate { id });
        }

        let parent_id = block.header.prev_hash;
        let parent = self.require(&parent_id)?;
        let height = parent.height + 1;
        let parent_work = parent.total_work;

        // The declared target must be exactly what the retarget rule produces.
        // Without this a miner could simply announce an easy target.
        let expected_target = self.next_target(&parent_id)?;
        if block.header.difficulty_target != expected_target {
            return Err(NodeError::Network(format!(
                "wrong difficulty target at height {height}: expected {}, got {}",
                hex::encode(expected_target),
                hex::encode(block.header.difficulty_target)
            )));
        }

        if self.config.verify_pow {
            // The rule is chosen by the height this block lands at, which is
            // derived from its parent — not from anything the block itself
            // declares. A miner cannot select an easier proof-of-work rule by
            // claiming to be somewhere else in the chain.
            let pow = block.header.pow_hash_at(height, &self.dag)?;
            if !meets_target(&pow, &block.header.difficulty_target) {
                return Err(NodeError::Network(format!(
                    "insufficient proof of work at height {height}"
                )));
            }
        }

        let total_work =
            parent_work.saturating_add(work_from_target(&block.header.difficulty_target));

        let record = BlockRecord {
            block,
            height,
            total_work,
            parent: Some(parent_id),
        };

        // Strictly greater: an equal-work branch leaves the incumbent tip in
        // place so peers cannot flip the chain back and forth for free.
        if total_work <= self.total_work() {
            self.records.insert(id, record);
            return Ok(InsertOutcome::SideBranch { id });
        }

        self.records.insert(id, record);

        if parent_id == self.tip {
            // Simple extension: no revert needed.
            match self.apply_one(&id) {
                Ok(()) => {
                    self.tip = id;
                    Ok(InsertOutcome::Extended { tip: id })
                }
                Err(e) => {
                    self.records.remove(&id);
                    Err(e)
                }
            }
        } else {
            match self.reorganize(&id) {
                Ok((reverted, applied)) => Ok(InsertOutcome::Reorganized {
                    tip: id,
                    reverted,
                    applied,
                }),
                Err(e) => {
                    self.records.remove(&id);
                    Err(e)
                }
            }
        }
    }

    /// Executes one block against state, journaling it for later revert.
    fn apply_one(&self, id: &BlockId) -> Result<()> {
        let record = self.require(id)?;
        // The record's own height, so timelocks evaluate against the block
        // actually being executed rather than the current tip.
        self.state.apply_block_journaled(
            &record.block,
            id,
            BlockContext::at_height(record.height),
        )?;
        Ok(())
    }

    /// Path from `id` back to (but excluding) `ancestor`, oldest first.
    fn path_from(&self, id: &BlockId, ancestor: &BlockId) -> Result<Vec<BlockId>> {
        let mut path = Vec::new();
        let mut cursor = *id;
        while cursor != *ancestor {
            path.push(cursor);
            match self.require(&cursor)?.parent {
                Some(parent) => cursor = parent,
                None => {
                    return Err(NodeError::Network(
                        "walked past genesis without reaching the common ancestor".to_string(),
                    ));
                }
            }
        }
        path.reverse();
        Ok(path)
    }

    /// Finds the deepest block common to both branches.
    fn common_ancestor(&self, left: &BlockId, right: &BlockId) -> Result<BlockId> {
        let mut a = *left;
        let mut b = *right;

        // Raise the deeper side until both are at equal height.
        while self.require(&a)?.height > self.require(&b)?.height {
            a = self.parent_of(&a)?;
        }
        while self.require(&b)?.height > self.require(&a)?.height {
            b = self.parent_of(&b)?;
        }

        // Step in lockstep until they meet.
        while a != b {
            a = self.parent_of(&a)?;
            b = self.parent_of(&b)?;
        }

        Ok(a)
    }

    fn parent_of(&self, id: &BlockId) -> Result<BlockId> {
        self.require(id)?.parent.ok_or_else(|| {
            NodeError::Network("reached genesis while seeking a common ancestor".to_string())
        })
    }

    /// Switches the active chain to the branch ending at `new_tip`.
    ///
    /// Returns `(reverted, applied)`. On failure the original branch is fully
    /// restored before the error propagates.
    fn reorganize(&mut self, new_tip: &BlockId) -> Result<(Vec<BlockId>, Vec<BlockId>)> {
        let ancestor = self.common_ancestor(&self.tip, new_tip)?;

        // Old branch newest-first, since undo journals must unwind in reverse.
        let mut to_revert = self.path_from(&self.tip, &ancestor)?;
        to_revert.reverse();
        let to_apply = self.path_from(new_tip, &ancestor)?;

        let mut reverted = Vec::new();
        for id in &to_revert {
            self.state.revert_block(id)?;
            reverted.push(*id);
        }

        let mut applied = Vec::new();
        for id in &to_apply {
            if let Err(error) = self.apply_one(id) {
                // Unwind this partial branch, then put the original chain back.
                self.restore_after_failed_reorg(&applied, &reverted);
                return Err(error);
            }
            applied.push(*id);
        }

        self.tip = *new_tip;
        Ok((reverted, applied))
    }

    /// Best-effort restoration after a reorg fails partway through.
    ///
    /// Reverts whatever the new branch managed to apply, then re-applies the
    /// original branch oldest-first. Errors here are not propagated: the caller
    /// is already returning the original failure, and there is no better
    /// recovery available than trying every step.
    fn restore_after_failed_reorg(&self, applied: &[BlockId], reverted: &[BlockId]) {
        for id in applied.iter().rev() {
            let _ = self.state.revert_block(id);
        }
        // `reverted` is newest-first, so replay it in reverse.
        for id in reverted.iter().rev() {
            let _ = self.apply_one(id);
        }
    }

    /// Block ids of the active chain, genesis first.
    ///
    /// # Errors
    ///
    /// Returns an error if the index is inconsistent.
    pub fn active_chain(&self) -> Result<Vec<BlockId>> {
        let mut chain = self.path_from(&self.tip, &self.genesis)?;
        chain.insert(0, self.genesis);
        Ok(chain)
    }

    /// Builds a candidate header for the next block on the active chain.
    ///
    /// `timestamp` is supplied by the caller so the result stays deterministic
    /// and testable.
    ///
    /// # Errors
    ///
    /// Returns an error if the tip is missing from the index.
    pub fn candidate_header(&self, timestamp: u64, state_root: [u8; 32]) -> Result<BlockHeader> {
        Ok(BlockHeader {
            prev_hash: self.tip,
            state_root,
            timestamp,
            nonce: 0,
            difficulty_target: self.next_target(&self.tip)?,
        })
    }

    /// Seconds the most recent full retarget window took.
    ///
    /// Returns `None` before the first complete window.
    #[must_use]
    pub fn last_window_timespan(&self) -> Option<u64> {
        let tip = self.records.get(&self.tip)?;
        if tip.height < RETARGET_INTERVAL {
            return None;
        }

        let mut cursor = tip;
        for _ in 0..RETARGET_INTERVAL {
            cursor = self.records.get(&cursor.parent?)?;
        }

        Some(
            tip.block
                .header
                .timestamp
                .saturating_sub(cursor.block.header.timestamp),
        )
    }

    /// The intended duration of a retarget window, for comparison against
    /// [`Chain::last_window_timespan`].
    #[must_use]
    pub fn expected_window_timespan() -> u64 {
        EXPECTED_TIMESPAN
    }
}
