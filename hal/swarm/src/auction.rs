//! Task micro-auctions: sealed bids, committed on chain, revealed after the
//! bidding closes, and each task given to its cheapest revealed bid, one task
//! per agent per round.
//!
//! The chain holds only commitments and the outcome; bids are revealed
//! against their commitments, so no agent can change its bid after seeing
//! others', and an unrevealed bid simply does not win.

use std::collections::{BTreeMap, BTreeSet};

use crate::{AgentId, SwarmError, TaskId};

/// A bid: what an agent asks to do a task, in the smallest currency unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bid {
    /// Bidder.
    pub agent: AgentId,
    /// Task.
    pub task: TaskId,
    /// Price asked.
    pub price: u64,
}

/// `BLAKE3(bid ‖ salt)`, domain-separated.
#[must_use]
pub fn commit(bid: &Bid, salt: &[u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new_derive_key("maya2c swarm task bid v1");
    h.update(&bid.agent.to_le_bytes());
    h.update(&bid.task.to_le_bytes());
    h.update(&bid.price.to_le_bytes());
    h.update(salt);
    *h.finalize().as_bytes()
}

/// One auction round.
#[derive(Clone, Debug, Default)]
pub struct Auction {
    tasks: BTreeSet<TaskId>,
    commitments: BTreeMap<(AgentId, TaskId), [u8; 32]>,
    revealed: Vec<Bid>,
    closed: bool,
}

impl Auction {
    /// A round offering `tasks`.
    #[must_use]
    pub fn new(tasks: impl IntoIterator<Item = TaskId>) -> Self {
        Self {
            tasks: tasks.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Records a sealed bid. One per agent per task.
    ///
    /// # Errors
    ///
    /// Bidding closed, an unknown task, or a second bid.
    pub fn seal(
        &mut self,
        agent: AgentId,
        task: TaskId,
        commitment: [u8; 32],
    ) -> Result<(), SwarmError> {
        if self.closed {
            return Err(SwarmError::Refused("bidding is closed"));
        }
        if !self.tasks.contains(&task) {
            return Err(SwarmError::Refused("no such task"));
        }
        if self.commitments.insert((agent, task), commitment).is_some() {
            return Err(SwarmError::Refused("one bid per agent per task"));
        }
        Ok(())
    }

    /// Ends bidding; reveals may start.
    pub fn close(&mut self) {
        self.closed = true;
    }

    /// Reveals a bid against its commitment.
    ///
    /// # Errors
    ///
    /// Bidding still open, or a bid that does not open its commitment.
    pub fn reveal(&mut self, bid: Bid, salt: &[u8; 32]) -> Result<(), SwarmError> {
        if !self.closed {
            return Err(SwarmError::Refused("reveal before bidding closed"));
        }
        match self.commitments.get(&(bid.agent, bid.task)) {
            Some(c) if *c == commit(&bid, salt) => {
                self.revealed.push(bid);
                Ok(())
            }
            _ => Err(SwarmError::Refused("the bid does not open its commitment")),
        }
    }

    /// The assignment: cheapest revealed bids first, each agent and each task
    /// used once; ties go to the lower agent id. Deterministic, so every
    /// node computes the same outcome.
    #[must_use]
    pub fn award(&self) -> BTreeMap<TaskId, (AgentId, u64)> {
        let mut bids = self.revealed.clone();
        bids.sort_by_key(|b| (b.price, b.agent, b.task));
        let mut busy = BTreeSet::new();
        let mut out = BTreeMap::new();
        for b in bids {
            if !out.contains_key(&b.task) && busy.insert(b.agent) {
                out.insert(b.task, (b.agent, b.price));
            }
        }
        out
    }
}
