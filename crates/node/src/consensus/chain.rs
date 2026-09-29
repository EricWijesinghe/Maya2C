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
use crate::core::{Block, BlockHeader, Transaction};
use crate::crypto::dag::registry::{CacheRegistry, DagConfig};
use crate::crypto::pow::meets_target;
use crate::error::{NodeError, Result};
use crate::state::{BlockContext, StateDB};
use crate::upgrade::{ProtocolUpgrade, UpgradeSchedule, refuse_unsupported};

/// Opening a stored chain, importing headers ahead of bodies, and pruning.
mod store;

/// Content-addressed block identifier. See [`BlockHeader::id`].
pub type BlockId = [u8; 32];

/// CON-8 (`spec/05-consensus.md`): a block at or below the prune horizon is
/// refused. Named so the conformance vectors reach the same predicate the
/// chain applies.
#[must_use]
pub const fn below_prune_horizon(height: u64, horizon: u64) -> bool {
    height <= horizon
}

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
    /// The first scheduled protocol upgrade this binary does not implement,
    /// from genesis (`UpgradeSchedule::first_unsupported`). A block at or past
    /// its height is refused with `UpgradeRequired` rather than validated
    /// under the old rules. Only this one entry matters to validation, which
    /// is what keeps the config `Copy`.
    pub unsupported_upgrade: Option<ProtocolUpgrade>,
    /// Keep every block at its parent's difficulty target: no retarget and no
    /// DAG activation pin. Set for DAG-BFT ([`ChainConfig::dag_bft`]), where
    /// no work is verified and a retarget only did harm: one-second blocks
    /// hardened the target every window until `total_work` saturated at
    /// 2^256 - 1, and every later block became a side branch (ADR-035).
    pub fixed_target: bool,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            verify_pow: true,
            pow_limit: default_pow_limit(),
            dag: DagConfig::MAINNET,
            unsupported_upgrade: None,
            fixed_target: false,
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
            unsupported_upgrade: None,
            fixed_target: false,
        }
    }

    /// Configuration for a DAG-BFT chain: no work verified, and the target
    /// fixed at genesis so total work grows by a constant per block.
    ///
    /// A block is derived from certificates by the node itself, never
    /// imported, so the target certifies nothing. A retarget still ran before
    /// ADR-035, and on one-second blocks it drove `total_work` to saturation
    /// within about 12,500 blocks, after which the chain could not extend.
    #[must_use]
    pub fn dag_bft() -> Self {
        Self {
            fixed_target: true,
            ..Self::without_pow_verification()
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
            unsupported_upgrade: None,
            fixed_target: false,
        }
    }

    /// The same configuration with a protocol upgrade schedule.
    #[must_use]
    pub fn with_upgrades(mut self, upgrades: &UpgradeSchedule) -> Self {
        self.unsupported_upgrade = upgrades.first_unsupported();
        self
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

/// A block's header plus the index metadata derived from it.
///
/// Header only. The body lives in the block store (`state::blocks`) and is
/// read when it is applied or served. Holding every body in memory grew
/// without bound, and a pruned node has no body at all below its horizon.
#[derive(Clone, Debug)]
pub struct BlockRecord {
    /// The block's header.
    pub header: BlockHeader,
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
    /// Highest height whose body and undo journal have been pruned; zero on a
    /// node that has pruned nothing. No reorg may reach below it.
    prune_horizon: u64,
}

impl Chain {
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

        if self.config.fixed_target {
            return Ok(super::difficulty::dag_bft_target(
                &parent.header.difficulty_target,
            ));
        }

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
            return Ok(parent.header.difficulty_target);
        }

        // Walk back RETARGET_INTERVAL blocks to find the window's first block.
        let mut cursor = parent;
        for _ in 0..RETARGET_INTERVAL.saturating_sub(1) {
            match cursor.parent.and_then(|id| self.records.get(&id)) {
                Some(previous) => cursor = previous,
                // Window extends past genesis: not enough history to retarget.
                None => return Ok(parent.header.difficulty_target),
            }
        }

        // Saturating: a parent timestamp behind the window start would
        // otherwise underflow. Zero clamps up to the minimum band below.
        let actual_timespan = parent
            .header
            .timestamp
            .saturating_sub(cursor.header.timestamp);

        Ok(retarget(
            &parent.header.difficulty_target,
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
    /// target is wrong, the transactions are not the ones the header's
    /// `tx_root` commits to, the proof of work is insufficient, applying the
    /// block's transactions fails, or they execute to a state root other than
    /// the one the header declares.
    pub fn insert_block(&mut self, block: Block) -> Result<InsertOutcome> {
        // First, before anything is stored — side branches included — and
        // before the duplicate check. Records are keyed by header id, so a
        // mismatched body stored under an honest header would make the genuine
        // block a `Duplicate` from then on: one bad relay would be enough to
        // censor it. And ahead of the duplicate check, an honest id carrying
        // someone else's transactions is reported as the error it is rather
        // than waved through as a block already held. Hashing the body is far
        // cheaper than the Argon2 pass below.
        block.check_tx_root()?;

        let id = block.header.id();
        // A record without a body is a header validated ahead of it, as a
        // pruned node's bootstrap imports them. Its body arriving is not a
        // duplicate: it goes through validation like any block, and lands.
        if self.records.contains_key(&id) && self.state.has_body(&id)? {
            return Ok(InsertOutcome::Duplicate { id });
        }

        let parent_id = block.header.prev_hash;
        let parent = self.require(&parent_id)?;
        let height = parent.height + 1;
        let parent_work = parent.total_work;

        // Before any rule is applied: a height governed by a protocol version
        // this binary does not implement is not ours to judge (spec/ §4).
        refuse_unsupported(self.config.unsupported_upgrade, height)?;

        // At or below the horizon the parent's undo journal is gone, so a
        // block forking there could never be reorganised onto. Refused before
        // it costs a proof-of-work check or a byte of storage.
        if below_prune_horizon(height, self.prune_horizon) {
            return Err(NodeError::BelowPruneHorizon {
                height,
                horizon: self.prune_horizon,
            });
        }

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

        // Stored before it is indexed, side branch or not, so a restart finds
        // every block the index held. A block that then fails to apply is
        // deleted again below.
        self.state.store_block(&block, height, total_work)?;
        let record = BlockRecord {
            header: block.header.clone(),
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

        let outcome = if parent_id == self.tip {
            // Simple extension: no revert needed, and the block is in hand, so
            // it is applied without reading back what was just stored.
            self.apply_body(&id, &block).map(|()| {
                self.tip = id;
                InsertOutcome::Extended { tip: id }
            })
        } else {
            self.reorganize(&id)
                .map(|(reverted, applied)| InsertOutcome::Reorganized {
                    tip: id,
                    reverted,
                    applied,
                })
        };
        if outcome.is_err() {
            self.records.remove(&id);
            // Best effort: the error being returned is the one that matters,
            // and a stray stored block is only reloaded as a side branch that
            // fails the same way again.
            let _ = self.state.delete_block(&id);
        }
        outcome
    }

    /// Executes one block against state, journaling it for later revert, and
    /// makes it the tip in the same batch.
    ///
    /// Refuses a block whose declared state root is not what it executes to —
    /// [`StateDB::apply_block_journaled`] is the checked path.
    fn apply_one(&self, id: &BlockId) -> Result<()> {
        let block = self.state.load_block(id)?.ok_or_else(|| {
            NodeError::Storage(format!("no body held for block {}", hex::encode(id)))
        })?;
        self.apply_body(id, &block)
    }

    /// [`Chain::apply_one`] for a block whose body is already in hand.
    fn apply_body(&self, id: &BlockId, block: &Block) -> Result<()> {
        let height = self.require(id)?.height;
        // The record's own height, so timelocks evaluate against the block
        // actually being executed rather than the current tip.
        self.state
            .apply_canonical(block, id, height, BlockContext::at_height(height))?;
        Ok(())
    }

    /// Reverts the active-chain block `id`, making its parent the tip in the
    /// same batch.
    fn revert_one(&self, id: &BlockId) -> Result<()> {
        let record = self.require(id)?;
        let parent = record
            .parent
            .ok_or_else(|| NodeError::Network("genesis cannot be reverted".to_string()))?;
        self.state.revert_canonical(id, record.height, &parent)
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

        // Checked before anything is reverted, so a reorg that cannot finish
        // never starts. Reverting to an ancestor below the horizon would need
        // undo journals this node pruned.
        let ancestor_height = self.require(&ancestor)?.height;
        if ancestor_height < self.prune_horizon {
            return Err(NodeError::BelowPruneHorizon {
                height: ancestor_height,
                horizon: self.prune_horizon,
            });
        }

        // Old branch newest-first, since undo journals must unwind in reverse.
        let mut to_revert = self.path_from(&self.tip, &ancestor)?;
        to_revert.reverse();
        let to_apply = self.path_from(new_tip, &ancestor)?;

        let mut reverted = Vec::new();
        for id in &to_revert {
            if let Err(error) = self.revert_one(id) {
                // A revert that fails partway leaves the chain between
                // branches. Re-apply what was reverted, as a failed apply does.
                self.restore_after_failed_reorg(&[], &reverted);
                return Err(error);
            }
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
            let _ = self.revert_one(id);
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

    /// Builds a candidate block carrying `transactions` on the active chain,
    /// with nonce zero, ready to mine.
    ///
    /// Both roots are filled in: `tx_root` from the transactions, and
    /// `state_root` from executing them against the tip's state at the height
    /// the block would land at. `timestamp` is supplied by the caller so the
    /// result stays deterministic and testable.
    ///
    /// # Errors
    ///
    /// Returns an error if the tip is missing from the index, or if any of
    /// `transactions` would fail to apply.
    pub fn candidate_block(&self, timestamp: u64, transactions: Vec<Transaction>) -> Result<Block> {
        self.candidate_block_sealed(timestamp, transactions, 0)
    }

    /// [`Chain::candidate_block`] with the header's `nonce` set *before* the
    /// state root is computed. A DAG-BFT block's nonce is its seal, and the
    /// staking pass reads it (ADR-028), so a root computed with nonce 0 and a
    /// seal written afterwards would be a root no node reproduces.
    ///
    /// # Errors
    ///
    /// As [`Chain::candidate_block`].
    pub fn candidate_block_sealed(
        &self,
        timestamp: u64,
        transactions: Vec<Transaction>,
        nonce: u64,
    ) -> Result<Block> {
        let header = BlockHeader {
            prev_hash: self.tip,
            state_root: [0; 32],
            timestamp,
            nonce,
            difficulty_target: self.next_target(&self.tip)?,
            tx_root: [0; 32],
        };
        let mut block = Block::new(header, transactions);
        block.header.state_root = self
            .state
            .preview_root(&block, BlockContext::at_height(self.height() + 1))?;
        Ok(block)
    }

    /// The header of an empty [`Chain::candidate_block`] — what every miner in
    /// the repository mines today.
    ///
    /// # Errors
    ///
    /// As [`Chain::candidate_block`].
    pub fn candidate_header(&self, timestamp: u64) -> Result<BlockHeader> {
        Ok(self.candidate_block(timestamp, Vec::new())?.header)
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

        Some(tip.header.timestamp.saturating_sub(cursor.header.timestamp))
    }

    /// The intended duration of a retarget window, for comparison against
    /// [`Chain::last_window_timespan`].
    #[must_use]
    pub fn expected_window_timespan() -> u64 {
        EXPECTED_TIMESPAN
    }
}
