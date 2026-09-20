//! Moving per-shard record caches from one map to another.
//!
//! The brief called this inter-shard teleportation. What moves is each leaf's
//! in-memory cache of the records under its range — the working set a lane
//! keeps hot. State itself does not move: every leaf reads one database and
//! commits to one root.
//!
//! # Atomic
//!
//! [`ShardStores::apply`] checks the plan was built for the map the stores are
//! on before it touches a record, and from then on cannot fail: every record
//! is placed by the new map's own lookup, so it lands in exactly one leaf and in
//! key order. There is no half-moved state to observe or to roll back.
//!
//! That covers the stores, not the stores *and* the manager together. The
//! sequence is `observe`, `apply`, `commit`, and it holds only if nothing
//! calls `observe` between the last two: an interleaved tick makes `commit`
//! refuse the plan after `apply` has already moved the caches. Both take
//! `&mut`, so a caller owning the two keeps them in step by calling them in
//! order; one sharing them across threads must hold one lock across all three.
//!
//! # Batched
//!
//! Records leave an old leaf in key order and new leaves are key ranges, so the
//! records going from one old leaf to one new leaf are contiguous. Each such run
//! is one [`Relocation`], which is what a lane handing its range to another
//! would hand over in one message.
//!
//! # No proof
//!
//! Both sides of the handoff are this process. A proof of a relabel that the
//! receiver can redo would cost more than redoing it — see
//! [`crate::shard_manager`].

use alloc::vec::Vec;

use crate::error::{GraphError, Result};
use crate::shard::ShardId;

use super::map::{Prefix, ShardMap};
use super::policy::Rebalance;

/// One contiguous run of records moved from one leaf to another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Relocation {
    /// Leaf of the old map the records left.
    pub from: Prefix,
    /// Leaf of the new map they joined.
    pub to: Prefix,
    /// How many.
    pub records: usize,
}

/// A cache of records per leaf, each held in key order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardStores<R> {
    map: ShardMap,
    shards: Vec<Vec<([u8; 32], R)>>,
}

impl<R> ShardStores<R> {
    /// Empty caches for every leaf of `map`.
    #[must_use]
    pub fn new(map: ShardMap) -> Self {
        let shards = (0..map.shard_count()).map(|_| Vec::new()).collect();
        Self { map, shards }
    }

    /// The map the caches are laid out by.
    #[must_use]
    pub const fn map(&self) -> &ShardMap {
        &self.map
    }

    /// The records cached for `shard`, empty for a leaf the map lacks.
    #[must_use]
    pub fn records(&self, shard: ShardId) -> &[([u8; 32], R)] {
        self.shards.get(shard.index()).map_or(&[], Vec::as_slice)
    }

    /// Records across every leaf.
    #[must_use]
    pub fn record_count(&self) -> usize {
        self.shards.iter().map(Vec::len).sum()
    }

    /// The record under `key`.
    #[must_use]
    pub fn get(&self, key: &[u8; 32]) -> Option<&R> {
        let records = self.records(self.map.locate(key));
        records
            .binary_search_by(|(stored, _)| stored.cmp(key))
            .ok()
            .map(|index| &records[index].1)
    }

    /// Caches `record` under `key` in the leaf owning it, returning any record
    /// it replaced.
    pub fn insert(&mut self, key: [u8; 32], record: R) -> Option<R> {
        let shard = self.map.locate(&key).index();
        let records = self.shards.get_mut(shard)?;
        match records.binary_search_by(|(stored, _)| stored.cmp(&key)) {
            Ok(index) => Some(core::mem::replace(&mut records[index].1, record)),
            Err(index) => {
                records.insert(index, (key, record));
                None
            }
        }
    }

    /// Lays the caches out by `rebalance`'s new map.
    ///
    /// # Errors
    ///
    /// [`GraphError::StaleRebalance`] if the plan was built for another map.
    /// Nothing has moved when it is returned.
    pub fn apply(&mut self, rebalance: &Rebalance) -> Result<Vec<Relocation>> {
        if rebalance.from() != &self.map {
            return Err(GraphError::StaleRebalance);
        }
        let to = rebalance.to().clone();
        let old = core::mem::take(&mut self.shards);
        let mut shards: Vec<Vec<([u8; 32], R)>> =
            (0..to.shard_count()).map(|_| Vec::new()).collect();
        let mut relocations: Vec<Relocation> = Vec::new();

        for (from, records) in self.map.leaves().iter().zip(old) {
            for (key, record) in records {
                // `locate` returns an index below `to.shard_count()`, which is
                // `shards.len()`. Indexed rather than skipped on a miss: a
                // record silently left behind would be the one failure this
                // function exists to rule out.
                let target = to.locate(&key).index();
                let joined = to.leaves()[target];
                match relocations.last_mut() {
                    Some(run) if run.from == *from && run.to == joined => run.records += 1,
                    _ => relocations.push(Relocation {
                        from: *from,
                        to: joined,
                        records: 1,
                    }),
                }
                shards[target].push((key, record));
            }
        }

        self.map = to;
        self.shards = shards;
        Ok(relocations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shard_manager::metrics::TickSample;
    use crate::shard_manager::policy::{ScalingConfig, ShardManager};

    fn key(first: u32) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[..4].copy_from_slice(&first.to_be_bytes());
        out[31] = first as u8;
        out
    }

    fn plan_for(map: &ShardMap) -> Rebalance {
        let config = ScalingConfig {
            lane_capacity: 1,
            split_streak: 1,
            ..ScalingConfig::DEFAULT
        };
        let mut manager = ShardManager::new(config, map.clone()).expect("manager");
        let addresses: Vec<[u8; 32]> = map.leaves().iter().map(|leaf| key(leaf.bits())).collect();
        let transactions: Vec<&[[u8; 32]]> = addresses.iter().map(core::slice::from_ref).collect();
        let sample = TickSample::measure(map, &transactions, 0).expect("sample");
        manager
            .observe(&sample)
            .expect("observe")
            .expect("split due")
    }

    #[test]
    fn a_split_moves_every_record_once_and_in_key_order() {
        let map = ShardMap::uniform(2).expect("four");
        let mut stores = ShardStores::new(map.clone());
        for step in 0..500u32 {
            stores.insert(key(step.wrapping_mul(0x9E37_79B9)), step);
        }
        let before = stores.record_count();

        let relocations = stores.apply(&plan_for(&map)).expect("apply");

        assert_eq!(stores.map().shard_count(), 8);
        assert_eq!(stores.record_count(), before);
        assert_eq!(
            relocations.iter().map(|run| run.records).sum::<usize>(),
            before
        );
        for index in 0..8 {
            let shard = ShardId::new(index).expect("in range");
            let leaf = stores.map().prefix(shard).expect("leaf");
            let records = stores.records(shard);
            assert!(records.iter().all(|(k, _)| leaf.contains(k)));
            assert!(records.windows(2).all(|pair| pair[0].0 < pair[1].0));
        }
        for step in 0..500u32 {
            assert_eq!(
                stores.get(&key(step.wrapping_mul(0x9E37_79B9))),
                Some(&step)
            );
        }
    }

    #[test]
    fn a_plan_for_another_map_is_refused_before_anything_moves() {
        let map = ShardMap::uniform(2).expect("four");
        let other = ShardMap::uniform(3).expect("eight");
        let mut stores = ShardStores::new(map);
        stores.insert(key(7), 7u32);
        let untouched = stores.clone();
        assert_eq!(
            stores.apply(&plan_for(&other)),
            Err(GraphError::StaleRebalance)
        );
        assert_eq!(stores, untouched);
    }
}
