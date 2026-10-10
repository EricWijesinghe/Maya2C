//! Mining channels: nonce ranges, per-channel state, and the registry.
//!
//! ## Disjoint nonce ranges are the whole design
//!
//! Bitcoin gives each miner its own search space through the coinbase
//! extranonce. `Maya2C` has no coinbase, so the only field a miner may vary is
//! the header's 8-byte nonce (`src/core/block.rs:23`). With tens of thousands
//! of channels on one job, every one of them would otherwise start at zero and
//! walk the same path — the pool would pay many miners for the same hash.
//!
//! Each channel is therefore assigned a disjoint half-open range at open time,
//! the way `src/consensus/miner.rs` already partitions across threads. Three
//! things follow, and all three are load-bearing:
//!
//! 1. Two channels cannot collide by construction rather than by luck.
//! 2. Duplicate detection is exact, and its memory is bounded by the range.
//! 3. A nonce outside a channel's range is refused **without hashing** — at
//!    25 ms a verification, the cheapest rejection the pool has.
//!
//! ## Ranges are recycled, and that is safe
//!
//! [`NonceAllocator`] returns a closed channel's range to a free list. Without
//! recycling, a pool with churn exhausts its ranges after a few million channel
//! opens and starts refusing connections for no reason a miner can see. It is
//! safe because duplicate detection is keyed by `(job, nonce)` *within* a
//! channel, and a recycled range belongs to a new channel with its own empty
//! set — there is no cross-channel duplicate to miss.

use std::collections::{HashMap, HashSet};

use custom_l1_node::crypto::pow::target_from_leading_zero_bits;

use crate::config::PoolConfig;
use crate::model::{RejectReason, WorkerKey};
use crate::vardiff::Vardiff;

/// Width of one channel's nonce range, as a power of two.
///
/// 2^40 nonces per channel against 2^24 ≈ 16.7 million concurrent channels.
/// The split is not arbitrary in either direction: a GPU at ~10^6 hashes per
/// second on the DAG rule takes over a decade to walk 2^40, so no honest rig
/// ever exhausts its range, and 16.7 million is two orders of magnitude above
/// the 50,000 connections `docs/stratum-v2.md` sizes for.
pub const NONCE_RANGE_BITS: u32 = 40;

/// Nonces in one range.
pub const NONCE_RANGE_SIZE: u64 = 1 << NONCE_RANGE_BITS;

/// Ranges available before the allocator is exhausted.
pub const MAX_CHANNELS: u64 = u64::MAX / NONCE_RANGE_SIZE;

/// Hands out disjoint nonce ranges.
#[derive(Debug, Default)]
pub struct NonceAllocator {
    /// Next never-issued range index.
    next_index: u64,
    /// Ranges returned by closed channels.
    free: Vec<u64>,
}

/// A half-open nonce range, `[min, max)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NonceRange {
    /// Index this range was allocated from, for recycling.
    index: u64,
    /// First nonce in the range.
    pub min: u64,
    /// One past the last nonce in the range.
    pub max: u64,
}

impl NonceRange {
    /// Whether `nonce` falls inside this range.
    #[must_use]
    pub fn contains(&self, nonce: u64) -> bool {
        nonce >= self.min && nonce < self.max
    }
}

impl NonceAllocator {
    /// A fresh allocator with every range available.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates a range, preferring one a closed channel returned.
    ///
    /// Returns `None` when every range is in use, which the caller reports as a
    /// channel-open error rather than by handing out an overlapping range.
    pub fn allocate(&mut self) -> Option<NonceRange> {
        let index = if let Some(recycled) = self.free.pop() {
            recycled
        } else {
            if self.next_index >= MAX_CHANNELS {
                return None;
            }
            let index = self.next_index;
            self.next_index += 1;
            index
        };

        Some(NonceRange {
            index,
            min: index * NONCE_RANGE_SIZE,
            max: (index + 1) * NONCE_RANGE_SIZE,
        })
    }

    /// Returns a range to the free list.
    pub fn release(&mut self, range: NonceRange) {
        self.free.push(range.index);
    }

    /// Ranges currently issued.
    #[must_use]
    pub fn issued(&self) -> u64 {
        self.next_index - self.free.len() as u64
    }
}

/// Everything the pool knows about one open channel.
#[derive(Debug)]
pub struct ChannelState {
    /// Channel id, unique across the pool for as long as the channel is open.
    pub id: u32,
    /// Who this channel mines for.
    pub key: WorkerKey,
    /// The nonce range it may search.
    pub range: NonceRange,
    /// Difficulty controller.
    pub vardiff: Vardiff,
    /// Hash rate the rig claimed. Unverified; see [`crate::model::WorkerStats`].
    pub reported_hashrate: f32,
    /// Job the channel is currently working, and the one before it.
    ///
    /// Two rather than one, and no more. At a 15-second block target a job goes
    /// stale fast (`docs/stratum-v2.md` §4), and a share for the immediately
    /// previous job is nearly always a share that was in flight when the job
    /// changed — refusing it would punish the miner for the pool's push
    /// latency. A share two jobs back is genuinely stale.
    pub current_job: Option<u32>,
    /// See [`ChannelState::current_job`].
    pub previous_job: Option<u32>,
    /// Nonces already submitted, per accepted job.
    ///
    /// Bounded by construction: at most two jobs are live, and each holds only
    /// the nonces this channel submitted for it.
    submitted: HashMap<u32, HashSet<u64>>,
    /// Highest sequence number acknowledged to the miner.
    pub last_acknowledged_sequence: u32,
    /// Accepted shares on this channel.
    pub accepted: u64,
    /// Rejected shares on this channel.
    pub rejected: u64,
    /// Stale shares, a subset of `rejected`.
    pub stale: u64,
    /// Accepted share weight on this channel.
    pub weight: u64,
}

impl ChannelState {
    /// Opens a channel at the configured starting difficulty.
    #[must_use]
    pub fn new(
        id: u32,
        key: WorkerKey,
        range: NonceRange,
        reported_hashrate: f32,
        config: &PoolConfig,
        now_millis: u64,
    ) -> Self {
        let mut vardiff = Vardiff::new(
            config.min_target_bits,
            config.min_target_bits,
            config.max_target_bits,
            config.share_interval,
        );
        // Seed the idle clock from the open, so a channel that never submits
        // anything is eased rather than sitting at its opening target forever.
        vardiff.on_accepted_share(now_millis);

        Self {
            id,
            key,
            range,
            vardiff,
            reported_hashrate,
            current_job: None,
            previous_job: None,
            submitted: HashMap::new(),
            last_acknowledged_sequence: 0,
            accepted: 0,
            rejected: 0,
            stale: 0,
            weight: 0,
        }
    }

    /// The channel's current share target, in header byte order.
    #[must_use]
    pub fn target(&self) -> [u8; 32] {
        target_from_leading_zero_bits(self.vardiff.bits())
    }

    /// Records a new job, retiring the one before last.
    pub fn push_job(&mut self, job_id: u32) {
        if let Some(retired) = self.previous_job {
            // The only place duplicate memory is released. Bounded because at
            // most two jobs are ever live on a channel.
            self.submitted.remove(&retired);
        }
        self.previous_job = self.current_job;
        self.current_job = Some(job_id);
        self.submitted.entry(job_id).or_default();
    }

    /// Whether `job_id` is one the channel may still submit for.
    #[must_use]
    pub fn accepts_job(&self, job_id: u32) -> bool {
        self.current_job == Some(job_id) || self.previous_job == Some(job_id)
    }

    /// Checks a submission against everything that costs nothing to check.
    ///
    /// Deliberately ordered cheapest-first, and deliberately ahead of any
    /// hashing: the point of the nonce range is that an out-of-range share
    /// never reaches a 25 ms verification.
    ///
    /// Returns `Ok(())` when the share is worth hashing.
    ///
    /// # Errors
    ///
    /// Returns the [`RejectReason`] the miner is told.
    pub fn precheck(&self, job_id: u32, nonce: u64) -> Result<(), RejectReason> {
        if !self.range.contains(nonce) {
            return Err(RejectReason::NonceOutOfRange);
        }
        if !self.accepts_job(job_id) {
            return Err(RejectReason::Stale);
        }
        if self
            .submitted
            .get(&job_id)
            .is_some_and(|nonces| nonces.contains(&nonce))
        {
            return Err(RejectReason::Duplicate);
        }
        Ok(())
    }

    /// Records a submission so a repeat of it is refused.
    ///
    /// Called for every submission that passed [`ChannelState::precheck`],
    /// whether or not the digest turned out to meet the target. A miner that
    /// resubmits a nonce which failed once will fail again, and hashing it a
    /// second time is 25 ms spent to learn nothing.
    pub fn remember(&mut self, job_id: u32, nonce: u64) {
        self.submitted.entry(job_id).or_default().insert(nonce);
    }

    /// Nonces remembered across every live job. Test and metrics use only.
    #[must_use]
    pub fn remembered(&self) -> usize {
        self.submitted.values().map(HashSet::len).sum()
    }
}

/// Every open channel on the pool.
#[derive(Debug, Default)]
pub struct ChannelRegistry {
    /// Open channels by id.
    channels: HashMap<u32, ChannelState>,
    /// Nonce range source.
    allocator: NonceAllocator,
    /// Next channel id.
    ///
    /// Monotonic and never recycled, unlike nonce ranges. A recycled channel id
    /// could be addressed by a message still in flight for the channel that
    /// held it, and that message would be applied to somebody else's rig.
    next_id: u32,
}

impl ChannelRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a channel, or `None` if ids or nonce ranges are exhausted.
    pub fn open(
        &mut self,
        key: WorkerKey,
        reported_hashrate: f32,
        config: &PoolConfig,
        now_millis: u64,
    ) -> Option<&ChannelState> {
        let range = self.allocator.allocate()?;
        let id = self.next_id.checked_add(1).map(|next| {
            let id = self.next_id;
            self.next_id = next;
            id
        })?;

        let state = ChannelState::new(id, key, range, reported_hashrate, config, now_millis);
        Some(self.channels.entry(id).or_insert(state))
    }

    /// Closes a channel and returns its nonce range to the pool.
    pub fn close(&mut self, id: u32) -> Option<ChannelState> {
        let state = self.channels.remove(&id)?;
        self.allocator.release(state.range);
        Some(state)
    }

    /// A channel by id.
    #[must_use]
    pub fn get(&self, id: u32) -> Option<&ChannelState> {
        self.channels.get(&id)
    }

    /// A channel by id, mutably.
    pub fn get_mut(&mut self, id: u32) -> Option<&mut ChannelState> {
        self.channels.get_mut(&id)
    }

    /// Every open channel.
    pub fn iter(&self) -> impl Iterator<Item = &ChannelState> {
        self.channels.values()
    }

    /// Every open channel, mutably.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut ChannelState> {
        self.channels.values_mut()
    }

    /// Open channel count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.channels.len()
    }

    /// Whether any channel is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn config() -> PoolConfig {
        PoolConfig {
            reward_per_block: 1_000,
            ..PoolConfig::default()
        }
    }

    fn key(worker: &str) -> WorkerKey {
        WorkerKey {
            miner: [7u8; 32],
            worker: worker.to_string(),
        }
    }

    #[test]
    fn allocated_ranges_never_overlap() {
        let mut allocator = NonceAllocator::new();
        let ranges: Vec<_> = (0..64).map(|_| allocator.allocate().unwrap()).collect();

        for (index, first) in ranges.iter().enumerate() {
            assert!(first.min < first.max, "range {index} is empty");
            for second in &ranges[index + 1..] {
                assert!(
                    first.max <= second.min || second.max <= first.min,
                    "ranges {first:?} and {second:?} overlap"
                );
            }
        }
    }

    #[test]
    fn a_released_range_is_handed_out_again() {
        // Without recycling, a pool with churn refuses connections after a few
        // million opens for no reason a miner can see.
        let mut allocator = NonceAllocator::new();
        let first = allocator.allocate().unwrap();
        allocator.release(first);
        assert_eq!(allocator.allocate().unwrap(), first);
        assert_eq!(allocator.issued(), 1);
    }

    #[test]
    fn a_nonce_outside_the_range_is_refused_before_anything_is_hashed() {
        let mut registry = ChannelRegistry::new();
        let id = registry.open(key("rig-1"), 1.0, &config(), 0).unwrap().id;
        let channel = registry.get_mut(id).unwrap();
        channel.push_job(9);

        let below = channel.range.min.wrapping_sub(1);
        assert_eq!(
            channel.precheck(9, below),
            Err(RejectReason::NonceOutOfRange)
        );
        assert_eq!(
            channel.precheck(9, channel.range.max),
            Err(RejectReason::NonceOutOfRange)
        );
        assert!(channel.precheck(9, channel.range.min).is_ok());
    }

    #[test]
    fn the_previous_job_is_still_accepted_but_the_one_before_it_is_not() {
        // Shares in flight when a job changes are the pool's push latency
        // showing up as the miner's rejection rate.
        let mut registry = ChannelRegistry::new();
        let id = registry.open(key("rig-1"), 1.0, &config(), 0).unwrap().id;
        let channel = registry.get_mut(id).unwrap();

        channel.push_job(1);
        channel.push_job(2);
        assert!(channel.accepts_job(1));
        assert!(channel.accepts_job(2));

        channel.push_job(3);
        assert!(!channel.accepts_job(1));
        assert!(channel.accepts_job(2));
    }

    #[test]
    fn a_repeated_nonce_is_refused() {
        let mut registry = ChannelRegistry::new();
        let id = registry.open(key("rig-1"), 1.0, &config(), 0).unwrap().id;
        let channel = registry.get_mut(id).unwrap();
        channel.push_job(1);

        let nonce = channel.range.min + 5;
        assert!(channel.precheck(1, nonce).is_ok());
        channel.remember(1, nonce);
        assert_eq!(channel.precheck(1, nonce), Err(RejectReason::Duplicate));
    }

    #[test]
    fn duplicate_memory_is_released_when_a_job_retires() {
        let mut registry = ChannelRegistry::new();
        let id = registry.open(key("rig-1"), 1.0, &config(), 0).unwrap().id;
        let channel = registry.get_mut(id).unwrap();

        channel.push_job(1);
        for offset in 0..100 {
            channel.remember(1, channel.range.min + offset);
        }
        assert_eq!(channel.remembered(), 100);

        channel.push_job(2);
        assert_eq!(channel.remembered(), 100, "job 1 is still live");
        channel.push_job(3);
        assert_eq!(channel.remembered(), 0, "job 1 should have been released");
    }

    #[test]
    fn closing_a_channel_returns_its_range() {
        let mut registry = ChannelRegistry::new();
        let id = registry.open(key("rig-1"), 1.0, &config(), 0).unwrap().id;
        assert_eq!(registry.len(), 1);

        registry.close(id).unwrap();
        assert!(registry.is_empty());

        // A new channel gets the recycled range but never the recycled id: a
        // message in flight for the old channel must not land on the new one.
        let reopened = registry.open(key("rig-2"), 1.0, &config(), 0).unwrap();
        assert_ne!(reopened.id, id);
    }

    #[test]
    fn every_open_channel_has_a_range_of_its_own() {
        let mut registry = ChannelRegistry::new();
        for index in 0..50 {
            registry
                .open(key(&format!("rig-{index}")), 1.0, &config(), 0)
                .expect("range available");
        }

        let mut ranges: Vec<_> = registry.iter().map(|channel| channel.range).collect();
        ranges.sort_by_key(|range| range.min);
        for pair in ranges.windows(2) {
            assert!(
                pair[0].max <= pair[1].min,
                "{:?} overlaps {:?}",
                pair[0],
                pair[1]
            );
        }
    }
}
