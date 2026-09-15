//! When to split and when to merge.
//!
//! # The rules
//!
//! - **Split** a leaf whose load has exceeded `split_permille` of one lane's
//!   capacity for `split_streak` consecutive ticks. One tick at or below the
//!   line resets the streak: "100 consecutive rounds" means consecutive.
//! - **Merge** two buddy leaves whose *combined* load has stayed under
//!   `merge_permille` of one lane for `merge_streak` ticks. Combined, so the
//!   merged leaf starts below the line; and `merge_permille < split_permille`
//!   is enforced, so a merge can never be the first half of a re-split.
//! - A leaf created by a rebalance does neither for `cooldown_ticks`.
//! - A split is vetoed while memory pressure is at or above
//!   `memory_veto_permille` (every leaf is a lane and a cache), and for a leaf
//!   whose load straddles its halves above `straddle_veto_permille` (see
//!   [`super::metrics`]).
//! - Splits and merges never share a tick, and the map stays within
//!   `min_shards..=max_shards`. Where the bound binds, the busiest leaves split
//!   first and the idlest pairs merge first, ties broken by key order, so a
//!   decision is a function of the sample and nothing else.
//!
//! # Plan, then commit
//!
//! [`ShardManager::observe`] proposes a [`Rebalance`] without adopting it. The
//! caller moves its caches ([`super::handoff`]) and then calls
//! [`ShardManager::commit`], which refuses a plan built for another map or
//! another tick. A handoff that fails leaves the manager on the old map. A
//! caller must not `observe` between `apply` and `commit`; see
//! [`super::handoff`] for why.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::error::{GraphError, Result};
use crate::shard::SHARD_COUNT;

use super::map::{MAX_PREFIX_BITS, Prefix, ShardMap};
use super::metrics::{PERMILLE, TickSample};

/// Thresholds and bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalingConfig {
    /// Transactions one leaf's lane executes per tick. The one figure that is
    /// a property of the machine; measure it rather than trust the default.
    pub lane_capacity: u32,
    /// Utilization a leaf must exceed to count towards a split.
    pub split_permille: u32,
    /// Consecutive hot ticks that trigger a split.
    pub split_streak: u32,
    /// Combined buddy utilization a pair must stay under to count towards a
    /// merge. Must be below `split_permille`.
    pub merge_permille: u32,
    /// Consecutive cold ticks that trigger a merge.
    pub merge_streak: u32,
    /// Ticks a new leaf waits before it may split or merge.
    pub cooldown_ticks: u64,
    /// Straddling share of a leaf's load above which it is not split.
    pub straddle_veto_permille: u32,
    /// Memory pressure at or above which nothing splits.
    pub memory_veto_permille: u16,
    /// Fewest leaves.
    pub min_shards: usize,
    /// Most leaves. At most [`SHARD_COUNT`].
    pub max_shards: usize,
}

impl ScalingConfig {
    /// The brief's thresholds — 80% for 100 ticks, 4 to 64 shards — with a
    /// merge line at half the split line and a merge window four times as
    /// long, so shrinking is slower than growing. `lane_capacity` is a
    /// placeholder, not a measurement.
    pub const DEFAULT: Self = Self {
        lane_capacity: 256,
        split_permille: 800,
        split_streak: 100,
        merge_permille: 400,
        merge_streak: 400,
        cooldown_ticks: 200,
        straddle_veto_permille: 500,
        memory_veto_permille: 900,
        min_shards: 4,
        max_shards: SHARD_COUNT,
    };

    /// Checks the configuration is one the policy can honour.
    ///
    /// # Errors
    ///
    /// [`GraphError::InvalidScalingConfig`] for a zero capacity or streak, a
    /// merge line at or above the split line, a veto above [`PERMILLE`], or
    /// bounds outside `1..=SHARD_COUNT` or inverted.
    pub const fn check(&self) -> Result<()> {
        let valid = self.lane_capacity > 0
            && self.split_permille > 0
            && self.merge_permille < self.split_permille
            && self.split_streak > 0
            && self.merge_streak > 0
            && self.straddle_veto_permille <= PERMILLE as u32
            && self.memory_veto_permille <= PERMILLE
            && self.min_shards >= 1
            && self.min_shards <= self.max_shards
            && self.max_shards <= SHARD_COUNT;
        if valid {
            Ok(())
        } else {
            Err(GraphError::InvalidScalingConfig)
        }
    }
}

impl Default for ScalingConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// One change to the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Bisect this leaf.
    Split(Prefix),
    /// Replace this range's two halves with the range.
    Merge(Prefix),
}

/// A proposed change from one map to another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rebalance {
    from: ShardMap,
    to: ShardMap,
    decisions: Vec<Decision>,
    tick: u64,
}

impl Rebalance {
    /// The map the plan was built against.
    #[must_use]
    pub const fn from(&self) -> &ShardMap {
        &self.from
    }

    /// The map the plan produces.
    #[must_use]
    pub const fn to(&self) -> &ShardMap {
        &self.to
    }

    /// What changed.
    #[must_use]
    pub fn decisions(&self) -> &[Decision] {
        &self.decisions
    }

    /// The tick it was proposed at.
    #[must_use]
    pub const fn tick(&self) -> u64 {
        self.tick
    }
}

/// Streaks and the current map.
#[derive(Clone, Debug)]
pub struct ShardManager {
    config: ScalingConfig,
    map: ShardMap,
    tick: u64,
    /// Consecutive hot ticks, per leaf.
    hot: BTreeMap<Prefix, u32>,
    /// Consecutive cold ticks, per buddy pair, keyed by the parent.
    cold: BTreeMap<Prefix, u32>,
    /// Tick a leaf was created at. Leaves of the starting map have none.
    born: BTreeMap<Prefix, u64>,
}

impl ShardManager {
    /// A manager starting on `map`.
    ///
    /// # Errors
    ///
    /// [`GraphError::InvalidScalingConfig`], or [`GraphError::ShardLimit`] if
    /// `map` is outside the configured bounds.
    pub fn new(config: ScalingConfig, map: ShardMap) -> Result<Self> {
        config.check()?;
        if map.shard_count() < config.min_shards || map.shard_count() > config.max_shards {
            return Err(GraphError::ShardLimit);
        }
        Ok(Self {
            config,
            map,
            tick: 0,
            hot: BTreeMap::new(),
            cold: BTreeMap::new(),
            born: BTreeMap::new(),
        })
    }

    /// The map in force.
    #[must_use]
    pub const fn map(&self) -> &ShardMap {
        &self.map
    }

    /// Ticks observed.
    #[must_use]
    pub const fn tick(&self) -> u64 {
        self.tick
    }

    /// The configuration.
    #[must_use]
    pub const fn config(&self) -> &ScalingConfig {
        &self.config
    }

    /// Records one tick and proposes a rebalance if one is due.
    ///
    /// # Errors
    ///
    /// [`GraphError::SampleMismatch`] for a sample taken over another map.
    pub fn observe(&mut self, sample: &TickSample) -> Result<Option<Rebalance>> {
        if sample.shard_count() != self.map.shard_count() {
            return Err(GraphError::SampleMismatch);
        }
        self.tick = self.tick.saturating_add(1);
        self.count_streaks(sample);

        let mut decisions = self.splits(sample);
        if decisions.is_empty() {
            decisions = self.merges(sample);
        }
        if decisions.is_empty() {
            return Ok(None);
        }
        Ok(Some(Rebalance {
            from: self.map.clone(),
            to: self.map_after(&decisions)?,
            decisions,
            tick: self.tick,
        }))
    }

    /// Adopts a proposal once its handoff has succeeded.
    ///
    /// # Errors
    ///
    /// [`GraphError::StaleRebalance`] for a plan built against another map or
    /// at another tick.
    pub fn commit(&mut self, rebalance: Rebalance) -> Result<()> {
        if rebalance.from != self.map || rebalance.tick != self.tick {
            return Err(GraphError::StaleRebalance);
        }
        let leaves = rebalance.to.leaves();
        for leaf in leaves {
            if !self.map.leaves().contains(leaf) {
                self.born.insert(*leaf, self.tick);
            }
        }
        let parents: Vec<Prefix> = rebalance.to.buddies().into_iter().map(|(_, p)| p).collect();
        self.hot.retain(|leaf, _| leaves.contains(leaf));
        self.born.retain(|leaf, _| leaves.contains(leaf));
        self.cold.retain(|parent, _| parents.contains(parent));
        self.map = rebalance.to;
        Ok(())
    }

    fn count_streaks(&mut self, sample: &TickSample) {
        let capacity = u64::from(self.config.lane_capacity);
        let load = sample.load();
        for (index, leaf) in self.map.leaves().iter().enumerate() {
            let hot = u64::from(load[index]) * u64::from(PERMILLE)
                > u64::from(self.config.split_permille) * capacity;
            bump(&mut self.hot, *leaf, hot);
        }
        for (left, parent) in self.map.buddies() {
            let combined = u64::from(load[left.index()]) + u64::from(load[left.index() + 1]);
            let cold =
                combined * u64::from(PERMILLE) < u64::from(self.config.merge_permille) * capacity;
            bump(&mut self.cold, parent, cold);
        }
    }

    fn cooled(&self, leaf: &Prefix) -> bool {
        self.born
            .get(leaf)
            .is_none_or(|born| self.tick >= born.saturating_add(self.config.cooldown_ticks))
    }

    fn splits(&self, sample: &TickSample) -> Vec<Decision> {
        if sample.memory_pressure_permille() >= self.config.memory_veto_permille {
            return Vec::new();
        }
        let room = self
            .config
            .max_shards
            .saturating_sub(self.map.shard_count());
        let (load, straddling) = (sample.load(), sample.straddling());
        let mut candidates: Vec<(u32, Prefix)> = self
            .map
            .leaves()
            .iter()
            .enumerate()
            .filter(|(index, leaf)| {
                let splits_apart = u64::from(straddling[*index]) * u64::from(PERMILLE)
                    <= u64::from(self.config.straddle_veto_permille) * u64::from(load[*index]);
                self.hot
                    .get(*leaf)
                    .is_some_and(|streak| *streak >= self.config.split_streak)
                    && self.cooled(leaf)
                    && leaf.depth() < MAX_PREFIX_BITS
                    && splits_apart
            })
            .map(|(index, leaf)| (load[index], *leaf))
            .collect();
        candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        candidates
            .into_iter()
            .take(room)
            .map(|(_, leaf)| Decision::Split(leaf))
            .collect()
    }

    fn merges(&self, sample: &TickSample) -> Vec<Decision> {
        let room = self
            .map
            .shard_count()
            .saturating_sub(self.config.min_shards);
        let load = sample.load();
        let leaves = self.map.leaves();
        let mut candidates: Vec<(u64, Prefix)> = self
            .map
            .buddies()
            .into_iter()
            .filter_map(|(left, parent)| {
                let index = left.index();
                let ready = self
                    .cold
                    .get(&parent)
                    .is_some_and(|streak| *streak >= self.config.merge_streak)
                    && self.cooled(&leaves[index])
                    && self.cooled(&leaves[index + 1]);
                ready.then(|| (u64::from(load[index]) + u64::from(load[index + 1]), parent))
            })
            .collect();
        candidates.sort_unstable();
        candidates
            .into_iter()
            .take(room)
            .map(|(_, parent)| Decision::Merge(parent))
            .collect()
    }

    fn map_after(&self, decisions: &[Decision]) -> Result<ShardMap> {
        let mut leaves = Vec::with_capacity(self.map.shard_count() + decisions.len());
        for leaf in self.map.leaves() {
            if decisions.contains(&Decision::Split(*leaf)) {
                let (low, high) = leaf.children()?;
                leaves.push(low);
                leaves.push(high);
                continue;
            }
            match leaf.parent() {
                Some(parent) if decisions.contains(&Decision::Merge(parent)) => {
                    if parent.children()?.0 == *leaf {
                        leaves.push(parent);
                    }
                }
                _ => leaves.push(*leaf),
            }
        }
        ShardMap::from_leaves(leaves)
    }
}

/// Advances a streak on `on`, resets it otherwise.
fn bump(streaks: &mut BTreeMap<Prefix, u32>, key: Prefix, on: bool) {
    let streak = streaks.entry(key).or_insert(0);
    *streak = if on { streak.saturating_add(1) } else { 0 };
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: ScalingConfig = ScalingConfig {
        lane_capacity: 10,
        ..ScalingConfig::DEFAULT
    };

    fn address(first: u32) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[..4].copy_from_slice(&first.to_be_bytes());
        out
    }

    /// `per_leaf[i]` single-address transactions spread across leaf `i`'s
    /// halves one at a time, so none straddles.
    fn sample(map: &ShardMap, per_leaf: &[u32], pressure: u16) -> TickSample {
        let addresses: Vec<[u8; 32]> = map
            .leaves()
            .iter()
            .zip(per_leaf)
            .flat_map(|(leaf, count)| {
                (0..*count).map(move |n| {
                    let offset = (leaf.span() / 2) * u64::from(n % 2);
                    address((leaf.start() + offset) as u32)
                })
            })
            .collect();
        let transactions: Vec<&[[u8; 32]]> = addresses.iter().map(core::slice::from_ref).collect();
        TickSample::measure(map, &transactions, pressure).expect("sample")
    }

    #[test]
    fn a_merge_line_at_or_above_the_split_line_is_refused() {
        let config = ScalingConfig {
            merge_permille: 800,
            ..CONFIG
        };
        assert_eq!(config.check(), Err(GraphError::InvalidScalingConfig));
        assert_eq!(ScalingConfig::DEFAULT.check(), Ok(()));
    }

    #[test]
    fn ninety_nine_hot_ticks_do_not_split_and_the_hundredth_does() {
        let map = ShardMap::uniform(2).expect("four");
        let mut manager = ShardManager::new(CONFIG, map.clone()).expect("manager");
        let hot = sample(&map, &[9, 5, 5, 5], 0);
        for _ in 0..99 {
            assert_eq!(manager.observe(&hot), Ok(None));
        }
        let plan = manager.observe(&hot).expect("observe").expect("split due");
        assert_eq!(plan.decisions(), &[Decision::Split(map.leaves()[0])]);
        assert_eq!(plan.to().shard_count(), 5);
    }

    #[test]
    fn one_tick_at_the_line_resets_the_streak() {
        let map = ShardMap::uniform(2).expect("four");
        let mut manager = ShardManager::new(CONFIG, map.clone()).expect("manager");
        let (hot, at_line) = (
            sample(&map, &[9, 5, 5, 5], 0),
            sample(&map, &[8, 5, 5, 5], 0),
        );
        for _ in 0..99 {
            manager.observe(&hot).expect("observe");
        }
        assert_eq!(manager.observe(&at_line), Ok(None));
        for _ in 0..99 {
            assert_eq!(manager.observe(&hot), Ok(None));
        }
        assert!(manager.observe(&hot).expect("observe").is_some());
    }

    #[test]
    fn a_stale_plan_or_a_foreign_sample_is_refused() {
        let map = ShardMap::uniform(2).expect("four");
        let mut manager = ShardManager::new(CONFIG, map.clone()).expect("manager");
        let hot = sample(&map, &[9, 9, 9, 9], 0);
        let mut plan = None;
        for _ in 0..100 {
            plan = manager.observe(&hot).expect("observe");
        }
        let plan = plan.expect("split due");
        manager.observe(&hot).expect("observe");
        assert_eq!(manager.commit(plan), Err(GraphError::StaleRebalance));
        let foreign = sample(&ShardMap::uniform(3).expect("eight"), &[0; 8], 0);
        assert_eq!(manager.observe(&foreign), Err(GraphError::SampleMismatch));
    }

    #[test]
    fn nothing_merges_below_the_minimum() {
        let map = ShardMap::uniform(2).expect("four");
        let mut manager = ShardManager::new(CONFIG, map.clone()).expect("manager");
        let idle = sample(&map, &[0, 0, 0, 0], 0);
        for _ in 0..2_000 {
            assert_eq!(manager.observe(&idle), Ok(None));
        }
    }
}
