//! Parallel execution that commits exactly what sequential execution would
//! (Master Prompt 12 §1, ADR-017).
//!
//! # The hard requirement
//!
//! For every block, the parallel result — final state, per-transaction gas,
//! success flags — is **byte-identical** to executing the block sequentially
//! in block order. Commit order is block order. `tests/differential.rs` checks
//! it on 100,000 random blocks across contention levels and thread counts.
//!
//! # Two schedulers
//!
//! - [`optimistic`] — execute every transaction in parallel against a
//!   best-guess view, then validate **in block order** by comparing each
//!   transaction's read *values* with the committed prefix. A transaction
//!   whose reads still hold is committed as executed; one whose reads went
//!   stale is re-executed on the spot against the committed prefix, which
//!   makes it correct by construction, and committed. One pass, no
//!   dependency graph, no declarations needed. Under contention it degrades
//!   gracefully to sequential plus overhead.
//! - [`waves`] — transactions *declare* their read and write sets (the
//!   workload generator provides them); a transaction joins the earliest wave
//!   after every earlier transaction it conflicts with, and each wave runs in
//!   parallel. Exact when declarations are honest; a transaction that touches
//!   an undeclared key fails deterministically instead of racing.
//!
//! This is not a full Block-STM (Gelashvili et al., `PPoPP` 2023): there is no
//! multi-version memory with per-incarnation estimates and no collaborative
//! scheduler. ADR-017 records why the simpler optimistic pass was measured
//! first and what would justify building the rest.
//!
//! # Semantics executed
//!
//! A small deterministic state machine over `u64` keys ([`exec`]): transfers
//! move balances with checked arithmetic, token transfers move token
//! balances, contract calls bump a storage counter, deploys write a code key,
//! and a transaction touching a hot key adds to it. `work` iterations of an
//! integer mix per transaction stand in for signature/VM cost so that
//! parallelism has something to parallelise; every figure states it.

#![warn(missing_docs)]

pub mod exec;
pub mod optimistic;
pub mod sequential;
pub mod waves;

use std::collections::BTreeMap;

pub use maya_loadgen::{Key, Tx};

/// Committed state: key → value. Absent means zero.
pub type State = BTreeMap<Key, u64>;

/// What one transaction did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Receipt {
    /// Whether it applied (a failed transfer is a no-op, not an error).
    pub success: bool,
    /// Gas charged: a function of the transaction and its final execution
    /// only, so it cannot depend on threads or re-executions.
    pub gas: u64,
}

/// A block's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockResult {
    /// Final value of every key the block wrote.
    pub writes: State,
    /// Per-transaction receipts, in block order.
    pub receipts: Vec<Receipt>,
}

/// How much parallel work was redone (for reports; not part of the result).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Transactions re-executed after their speculative reads went stale.
    pub reexecuted: usize,
    /// Waves (for the wave scheduler) or 1.
    pub waves: usize,
}

/// A root over a result, for comparing runs cheaply.
pub fn result_digest(r: &BlockResult) -> u64 {
    // FNV-1a over the canonical serialisation: order is the BTreeMap's.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: u64| {
        for b in x.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for (k, v) in &r.writes {
        eat(*k);
        eat(*v);
    }
    for rc in &r.receipts {
        eat(u64::from(rc.success));
        eat(rc.gas);
    }
    h
}
