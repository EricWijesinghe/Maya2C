//! Elastic shards: a per-node partition that bisects under load and merges
//! when idle.
//!
//! # A shard map here is an execution choice, not consensus
//!
//! The fixed partition in [`crate::shard`] is 64 prefixes of the address. This
//! module lets a node use a *different* tiling of the same address space — from
//! 4 leaves up to 64, deeper where it is hot — and change it while it runs.
//! That is safe to do locally, and only locally, because of what
//! [`crate::schedule`](fn@crate::schedule) already guarantees:
//!
//! - Two transactions naming a common address land in the same leaf under
//!   *every* tiling, so they conflict under every map and keep their order.
//! - Two transactions naming disjoint addresses may or may not share a leaf.
//!   Sharing one serialises them; not sharing lets them run together. Either
//!   way the waves reach the serial state.
//!
//! So the state root does not depend on the map, and inputs no other node can
//! see — how many threads this machine has, how much memory it has left — are
//! legitimate triggers. They would not be if the map were consensus: a split
//! on one node and not another would be a chain split, the failure invariant
//! 28 and execution directive 2 exist to prevent.
//!
//! Two consequences worth stating:
//!
//! - **The fixed [`crate::shard_of`] stays.** `src/neural_gas/features.rs`
//!   reads it to compute a fee feature, and a feature that moved with each
//!   node's load would be a consensus value that differs between nodes.
//! - **The guarantee rests on access sets naming every address a transaction
//!   touches.** That is the existing contract of [`crate::schedule`](fn@crate::schedule), and
//!   `core::batch::access_for_transaction` in the node meets it only for plain
//!   transfers today. With a set that is too narrow a per-node map is worse
//!   than a fixed one: two nodes on different maps group the same transactions
//!   into different waves, so the bug diverges between nodes rather than
//!   hiding on all of them alike. That set has to be fixed before any consensus
//!   path reaches the scheduler. See `docs/blockgraph.md`.
//!
//! # Pieces
//!
//! - [`map`]: the tiling, lookup, split and buddy merge.
//! - [`metrics`]: what one scheduling tick looked like.
//! - [`policy`]: when to split and when to merge.
//! - [`handoff`]: moving per-shard record caches to a new map.
//!
//! # "Teleportation" and why there is no proof
//!
//! A split or a merge relabels key ranges. All shards live in one process over
//! one state, so no record leaves the machine and nothing needs a proof: a
//! zero-knowledge proof that a relabel was done correctly would prove, at a cost
//! orders of magnitude above redoing it, a fact every node recomputes. The only
//! proof systems in this tree are also not post-quantum. What does move is each
//! shard's in-memory record cache, and [`handoff`] moves it atomically: a plan
//! built for another map is refused before anything is touched.
//!
//! # Rounds
//!
//! Narwhal's rounds come from certificates, and this chain has no committee
//! (`docs/blockgraph.md`). The unit here is the **tick**: one scheduled block.

pub mod handoff;
pub mod map;
pub mod metrics;
pub mod policy;

pub use handoff::{Relocation, ShardStores};
pub use map::{MAX_PREFIX_BITS, Prefix, ShardMap, key_bits};
pub use metrics::{PERMILLE, TickSample};
pub use policy::{Decision, Rebalance, ScalingConfig, ShardManager};
