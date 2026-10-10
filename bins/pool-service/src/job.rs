//! Work templates, and the ids miners submit against.
//!
//! ## Jobs go stale fast here
//!
//! `TARGET_BLOCK_TIME` is fifteen seconds (`src/consensus/difficulty.rs:32`).
//! A ten-minute chain can afford to measure its stale-grace window in seconds;
//! this one cannot. Job push has to be sub-second and the grace window is one
//! job deep — see [`crate::channel::ChannelState::current_job`] for why one and
//! not zero, and not three.
//!
//! ## The pool builds empty blocks
//!
//! `get_mining_candidate` returns a header over the current state root and no
//! transaction list (`src/rpc/server.rs:147`), and `submit_block` takes a whole
//! block. The pool therefore submits `Block::new(header, vec![])`, which is what
//! the node's own miner does. This is not a limitation being worked around: on
//! a chain that burns fees to `FEE_SINK` there is no fee revenue to collect by
//! including transactions, so template construction is the node's business and
//! not the pool's.

use std::collections::HashMap;

use custom_l1_node::core::BlockHeader;
use custom_l1_node::rpc::MiningCandidate;

use crate::error::{PoolError, Result};

/// Jobs retained for submissions.
///
/// Three: the live job, the one a share might still be in flight for, and one
/// slot of slack so a job is never evicted in the same instant it is retired.
const RETAINED_JOBS: usize = 3;

/// How far ahead of the template's own timestamp a miner's `ntime` may sit.
///
/// A miner rolls `ntime` forward while it searches, so some drift is normal and
/// refusing all of it would reject honest work. Sixty seconds is four block
/// times: far more than a rig needs, and far less than the two hours a header
/// timestamp could otherwise claim.
const MAX_NTIME_DRIFT_SECONDS: u64 = 60;

/// One unit of work handed to miners.
#[derive(Clone, Debug)]
pub struct Job {
    /// Id carried in `NewMiningJob` and quoted back in every share.
    pub id: u32,
    /// Height the solved block would occupy.
    ///
    /// Needed for verification, not decoration: `pow_hash_at` selects
    /// `ArgonBlake` or the DAG rule by height, and hashing at the wrong height
    /// produces a digest the chain would never accept.
    pub height: u64,
    /// The header to mine, with `nonce` at zero.
    pub header: BlockHeader,
    /// Target the chain requires for a block at this height.
    pub network_target: [u8; 32],
}

impl Job {
    /// Builds a job from an RPC mining candidate.
    ///
    /// # Errors
    ///
    /// Returns [`PoolError::NodeRpc`] if the candidate's header does not decode,
    /// which would mean the pool and the node disagree about header layout —
    /// the one disagreement that produces valid-looking work the chain rejects.
    pub fn from_candidate(id: u32, candidate: &MiningCandidate) -> Result<Self> {
        let bytes = hex::decode(&candidate.header_bytes)
            .map_err(|e| PoolError::NodeRpc(format!("candidate header is not hex: {e}")))?;
        let header = BlockHeader::from_bytes(&bytes)
            .map_err(|e| PoolError::NodeRpc(format!("candidate header did not decode: {e}")))?;

        Ok(Self {
            id,
            height: candidate.height,
            network_target: header.difficulty_target,
            header,
        })
    }

    /// Whether a miner's `ntime` is usable for this job.
    ///
    /// Rejects a timestamp before the template's own — that would be a header
    /// claiming to precede the state it commits to — and one implausibly far
    /// ahead.
    #[must_use]
    pub fn accepts_ntime(&self, ntime: u64) -> bool {
        ntime >= self.header.timestamp && ntime <= self.header.timestamp + MAX_NTIME_DRIFT_SECONDS
    }

    /// The header a submission claims to have solved.
    #[must_use]
    pub fn header_with(&self, nonce: u64, ntime: u64) -> BlockHeader {
        BlockHeader {
            nonce,
            timestamp: ntime,
            ..self.header
        }
    }
}

/// Jobs currently live on the pool.
#[derive(Debug, Default)]
pub struct JobRegistry {
    /// Retained jobs by id.
    jobs: HashMap<u32, Job>,
    /// Insertion order, oldest first, for eviction.
    order: Vec<u32>,
    /// The job new work is handed out on.
    current: Option<u32>,
    /// Next job id.
    next_id: u32,
}

impl JobRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a template and makes it current, evicting the oldest job.
    ///
    /// Returns `None` when the candidate describes the same work as the current
    /// job — same parent and same state root — and nothing was added.
    ///
    /// ## Why the check lives here and not in the caller
    ///
    /// `get_mining_candidate` stamps a fresh timestamp on every call
    /// (`src/rpc/server.rs:155`), so *every* poll returns a candidate that
    /// differs by a byte. A caller that pushed each one would issue a new job id
    /// twice a second, and with only `RETAINED_JOBS` retained, the id a
    /// channel is actually mining would be evicted about a second and a half
    /// later — after which every share that channel submitted would be refused
    /// as stale. The pool would look like it was rejecting all work from
    /// everyone, and the cause would be a job registry quietly forgetting the
    /// jobs it had handed out.
    ///
    /// Putting the check in the caller works right up until a second caller
    /// appears. Putting it here makes it a property of the registry.
    ///
    /// # Errors
    ///
    /// Propagates a candidate that does not decode.
    pub fn push(&mut self, candidate: &MiningCandidate) -> Result<Option<u32>> {
        let id = self.next_id;
        let job = Job::from_candidate(id, candidate)?;

        // Only a new parent, a new state root, or a new transaction set is
        // new work.
        if let Some(current) = self.current()
            && current.header.prev_hash == job.header.prev_hash
            && current.header.state_root == job.header.state_root
            && current.header.tx_root == job.header.tx_root
        {
            return Ok(None);
        }

        // Wrapping is correct and reachable: at one job per block and a
        // fifteen-second target, the space is exhausted after about two
        // thousand years, but a wrap must not panic if it ever happens. Ids
        // stay unique among *retained* jobs, which is the only place they are
        // compared.
        self.next_id = self.next_id.wrapping_add(1);

        self.jobs.insert(id, job);
        self.order.push(id);
        self.current = Some(id);

        while self.order.len() > RETAINED_JOBS {
            let evicted = self.order.remove(0);
            self.jobs.remove(&evicted);
        }

        Ok(Some(id))
    }

    /// The job new channels are put on.
    #[must_use]
    pub fn current(&self) -> Option<&Job> {
        self.current.and_then(|id| self.jobs.get(&id))
    }

    /// A retained job by id.
    #[must_use]
    pub fn get(&self, id: u32) -> Option<&Job> {
        self.jobs.get(&id)
    }

    /// Retained job count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    /// Whether the pool has any work to hand out.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
    use custom_l1_node::rpc::HeaderInfo;

    /// A candidate whose parent hash follows its height.
    ///
    /// Distinct parents matter now that `push` deduplicates: two candidates
    /// with the same parent and state root *are* the same work, however far
    /// apart their timestamps or heights.
    fn candidate(height: u64, timestamp: u64) -> MiningCandidate {
        let header = BlockHeader {
            prev_hash: [height as u8; 32],
            state_root: [2u8; 32],
            timestamp,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(20),
            tx_root: [0; 32],
        };
        MiningCandidate {
            height,
            difficulty_target: hex::encode(header.difficulty_target),
            header_bytes: hex::encode(header.serialize()),
            header: HeaderInfo::from(&header),
        }
    }

    #[test]
    fn a_job_carries_the_height_verification_needs() {
        // `pow_hash_at` picks ArgonBlake or the DAG by height. A job that lost
        // its height would be verified under the wrong rule.
        let mut registry = JobRegistry::new();
        let id = registry
            .push(&candidate(30_001, 1_700_000_000))
            .unwrap()
            .expect("new work");
        assert_eq!(registry.get(id).unwrap().height, 30_001);
    }

    #[test]
    fn the_newest_job_becomes_current() {
        let mut registry = JobRegistry::new();
        registry
            .push(&candidate(1, 1_700_000_000))
            .unwrap()
            .expect("new work");
        let second = registry
            .push(&candidate(2, 1_700_000_015))
            .unwrap()
            .expect("new work");
        assert_eq!(registry.current().unwrap().id, second);
    }

    #[test]
    fn a_candidate_that_only_moved_its_timestamp_is_not_new_work() {
        // `get_mining_candidate` stamps a fresh timestamp on every call, so a
        // registry that took each one at face value would issue a job id twice
        // a second.
        let mut registry = JobRegistry::new();
        let first = registry
            .push(&candidate(1, 1_700_000_000))
            .unwrap()
            .expect("the first candidate is new work");

        assert_eq!(registry.push(&candidate(1, 1_700_000_001)).unwrap(), None);
        assert_eq!(registry.push(&candidate(1, 1_700_000_002)).unwrap(), None);
        assert_eq!(registry.current().unwrap().id, first);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn a_job_a_channel_is_mining_is_not_evicted_by_repeated_polling() {
        // The failure this pins: with a job id issued per poll, the id a rig was
        // actually working would fall out of the retention window in about a
        // second and a half, and every share that rig submitted afterwards
        // would come back as `stale-share`. The pool would look like it was
        // rejecting all work from everyone.
        let mut registry = JobRegistry::new();
        let handed_out = registry
            .push(&candidate(1, 1_700_000_000))
            .unwrap()
            .expect("new work");

        // Two polls a second for a minute, with nothing new on the chain.
        for tick in 0..120 {
            registry.push(&candidate(1, 1_700_000_000 + tick)).unwrap();
        }

        assert!(
            registry.get(handed_out).is_some(),
            "the job miners are working was evicted by polling alone"
        );
    }

    #[test]
    fn only_a_bounded_number_of_jobs_is_retained() {
        // Unbounded retention is a slow leak on a chain producing four jobs a
        // minute.
        let mut registry = JobRegistry::new();
        for height in 0..20 {
            registry
                .push(&candidate(height, 1_700_000_000 + height * 15))
                .unwrap();
        }
        assert_eq!(registry.len(), RETAINED_JOBS);
    }

    #[test]
    fn an_ntime_before_the_template_is_refused() {
        // A header claiming to precede the state root it commits to.
        let mut registry = JobRegistry::new();
        let id = registry
            .push(&candidate(1, 1_700_000_000))
            .unwrap()
            .expect("new work");
        let job = registry.get(id).unwrap();

        assert!(!job.accepts_ntime(1_699_999_999));
        assert!(job.accepts_ntime(1_700_000_000));
    }

    #[test]
    fn an_ntime_far_in_the_future_is_refused() {
        let mut registry = JobRegistry::new();
        let id = registry
            .push(&candidate(1, 1_700_000_000))
            .unwrap()
            .expect("new work");
        let job = registry.get(id).unwrap();

        assert!(job.accepts_ntime(1_700_000_000 + MAX_NTIME_DRIFT_SECONDS));
        assert!(!job.accepts_ntime(1_700_000_000 + MAX_NTIME_DRIFT_SECONDS + 1));
    }

    #[test]
    fn a_candidate_the_pool_cannot_decode_is_an_rpc_error() {
        // The pool and node disagreeing on header layout is the failure that
        // produces valid-looking work the chain rejects, so it must be loud.
        let mut broken = candidate(1, 1_700_000_000);
        broken.header_bytes = "00ff".to_string();

        let mut registry = JobRegistry::new();
        assert!(matches!(registry.push(&broken), Err(PoolError::NodeRpc(_))));
    }
}
