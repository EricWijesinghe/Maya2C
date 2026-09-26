//! The two work modes as a fork-choice engine: most cumulative work wins.
//!
//! `argonblake-pow` and `pouw-lattice` order blocks the same way and differ
//! only in the work function, which is verified elsewhere (`crypto::argon_blake`
//! in the node, `crates/lattice-pow`). In the simulator *who* finds the next
//! block is drawn by the caller in proportion to hash power, which is what a
//! work lottery is statistically; this module decides only what each node
//! does with the blocks it sees. That is the part that must be identical
//! across modes and deterministic across nodes.

use std::collections::BTreeMap;

use crate::mode::ConsensusMode;
use crate::vertex::Digest;

/// A block in a work chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkBlock {
    /// Parent digest; all-zero for genesis's children.
    pub parent: Digest,
    /// Height (genesis children are 1).
    pub height: u64,
    /// Miner.
    pub miner: u16,
    /// Work this block claims (its difficulty), already verified.
    pub work: u128,
    /// Transactions.
    pub batch: Vec<u64>,
    /// Which work mode produced it; part of the digest so the two modes'
    /// chains can never be confused.
    pub mode: ConsensusMode,
}

impl WorkBlock {
    /// Content digest.
    pub fn digest(&self) -> Digest {
        let mut h = blake3::Hasher::new();
        h.update(b"maya2c/work-block/v1");
        h.update(self.mode.name().as_bytes());
        h.update(&self.parent);
        h.update(&self.height.to_le_bytes());
        h.update(&self.miner.to_le_bytes());
        h.update(&self.work.to_le_bytes());
        for tx in &self.batch {
            h.update(&tx.to_le_bytes());
        }
        *h.finalize().as_bytes()
    }
}

/// One node's view of a work chain.
#[derive(Clone, Debug)]
pub struct ForkChoice {
    mode: ConsensusMode,
    blocks: BTreeMap<Digest, (WorkBlock, u128)>,
    orphans: BTreeMap<Digest, Vec<WorkBlock>>,
    tip: Digest,
    tip_work: u128,
}

/// The parent of the first block.
pub const GENESIS: Digest = [0u8; 32];

impl ForkChoice {
    /// An empty chain for a work mode.
    pub fn new(mode: ConsensusMode) -> Self {
        Self {
            mode,
            blocks: BTreeMap::new(),
            orphans: BTreeMap::new(),
            tip: GENESIS,
            tip_work: 0,
        }
    }

    /// The mode this chain runs.
    pub fn mode(&self) -> ConsensusMode {
        self.mode
    }

    /// Current best tip.
    pub fn tip(&self) -> Digest {
        self.tip
    }

    /// Height of the best tip.
    pub fn height(&self) -> u64 {
        self.blocks.get(&self.tip).map_or(0, |(b, _)| b.height)
    }

    /// Adds a block (buffering it if its parent is unknown). Returns whether
    /// the best tip changed. Ties keep the incumbent tip, so a node never
    /// flaps between equal-work chains.
    pub fn add(&mut self, block: WorkBlock) -> bool {
        if block.mode != self.mode {
            return false;
        }
        let before = self.tip;
        let mut queue = vec![block];
        while let Some(b) = queue.pop() {
            let digest = b.digest();
            if self.blocks.contains_key(&digest) {
                continue;
            }
            let parent_work = if b.parent == GENESIS {
                Some(0)
            } else {
                self.blocks.get(&b.parent).map(|(_, w)| *w)
            };
            let Some(parent_work) = parent_work else {
                self.orphans.entry(b.parent).or_default().push(b);
                continue;
            };
            let total = parent_work.saturating_add(b.work);
            if total > self.tip_work {
                self.tip = digest;
                self.tip_work = total;
            }
            self.blocks.insert(digest, (b, total));
            if let Some(children) = self.orphans.remove(&digest) {
                queue.extend(children);
            }
        }
        before != self.tip
    }

    /// The best chain from genesis, oldest first.
    pub fn best_chain(&self) -> Vec<&WorkBlock> {
        let mut chain = Vec::new();
        let mut cursor = self.tip;
        while let Some((b, _)) = self.blocks.get(&cursor) {
            chain.push(b);
            cursor = b.parent;
        }
        chain.reverse();
        chain
    }

    /// Blocks at least `depth` below the tip: probabilistic finality.
    pub fn confirmed(&self, depth: u64) -> Vec<&WorkBlock> {
        let height = self.height();
        self.best_chain()
            .into_iter()
            .filter(|b| b.height + depth <= height)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(parent: Digest, height: u64, miner: u16, work: u128) -> WorkBlock {
        WorkBlock {
            parent,
            height,
            miner,
            work,
            batch: vec![],
            mode: ConsensusMode::ArgonBlakePow,
        }
    }

    #[test]
    fn heavier_chain_wins_and_reorgs_even_when_shorter() {
        let mut fc = ForkChoice::new(ConsensusMode::ArgonBlakePow);
        let a1 = block(GENESIS, 1, 0, 10);
        let a2 = block(a1.digest(), 2, 0, 10);
        fc.add(a1.clone());
        fc.add(a2.clone());
        assert_eq!(fc.tip(), a2.digest());
        let b1 = block(GENESIS, 1, 1, 25);
        assert!(fc.add(b1.clone()));
        assert_eq!(
            fc.tip(),
            b1.digest(),
            "25 work beats 20 work at lower height"
        );
    }

    #[test]
    fn orphans_attach_when_their_parent_arrives() {
        let mut fc = ForkChoice::new(ConsensusMode::PouwLattice);
        let mut a1 = block(GENESIS, 1, 0, 1);
        a1.mode = ConsensusMode::PouwLattice;
        let mut a2 = block(a1.digest(), 2, 0, 1);
        a2.mode = ConsensusMode::PouwLattice;
        assert!(!fc.add(a2.clone()));
        assert!(fc.add(a1));
        assert_eq!(fc.tip(), a2.digest());
    }

    #[test]
    fn a_block_from_the_other_work_mode_is_ignored() {
        let mut fc = ForkChoice::new(ConsensusMode::PouwLattice);
        assert!(!fc.add(block(GENESIS, 1, 0, 1_000)));
        assert_eq!(fc.height(), 0);
    }
}
