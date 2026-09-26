//! Optimistic parallel execution with in-order value validation.
//!
//! 1. Every transaction executes in parallel against `base` (the pre-block
//!    state), recording the value of every key it read.
//! 2. In block order, each transaction's recorded reads are compared with the
//!    committed prefix (base plus the writes of every transaction already
//!    committed). If every value matches, its writes are exactly what a
//!    sequential run would produce at this position, and they are committed
//!    unchanged. If any differs, it is re-executed right there against the
//!    committed prefix — correct by construction — and committed.
//!
//! The committed result is therefore the sequential result, whatever the
//! parallel phase guessed; the parallel phase only decides how much work
//! step 2 has to redo.

use std::collections::BTreeMap;

use crate::exec::{Trace, run};
use crate::{BlockResult, State, Stats, Tx};

/// Executes `txs` over `base` with `threads` workers.
pub fn execute(base: &State, txs: &[Tx], threads: usize, work: u32) -> (BlockResult, Stats) {
    let threads = threads.max(1);
    let speculative = speculate(base, txs, threads, work);

    let mut committed: State = State::new();
    let mut receipts = Vec::with_capacity(txs.len());
    let mut stats = Stats {
        reexecuted: 0,
        waves: 1,
    };
    let view = |committed: &State, k| {
        committed
            .get(&k)
            .or_else(|| base.get(&k))
            .copied()
            .unwrap_or(0)
    };
    for (tx, spec) in txs.iter().zip(speculative) {
        let still_valid = spec.reads.iter().all(|(k, v)| view(&committed, *k) == *v);
        let trace = if still_valid {
            spec
        } else {
            stats.reexecuted += 1;
            run(tx, work, |k| view(&committed, k))
        };
        for (k, v) in &trace.writes {
            committed.insert(*k, *v);
        }
        receipts.push(trace.receipt);
    }
    (
        BlockResult {
            writes: committed,
            receipts,
        },
        stats,
    )
}

/// Phase 1: run every transaction against `base`, in parallel.
fn speculate(base: &State, txs: &[Tx], threads: usize, work: u32) -> Vec<Trace> {
    // Spawning costs more than a handful of transactions; below two per
    // worker, run inline. Same result either way.
    if threads == 1 || txs.len() < 2 * threads {
        return txs
            .iter()
            .map(|tx| run(tx, work, |k| base.get(&k).copied().unwrap_or(0)))
            .collect();
    }
    let chunk = txs.len().div_ceil(threads);
    let mut out: BTreeMap<usize, Vec<Trace>> = BTreeMap::new();
    std::thread::scope(|s| {
        let handles: Vec<_> = txs
            .chunks(chunk)
            .enumerate()
            .map(|(i, part)| {
                s.spawn(move || {
                    let traces: Vec<Trace> = part
                        .iter()
                        .map(|tx| run(tx, work, |k| base.get(&k).copied().unwrap_or(0)))
                        .collect();
                    (i, traces)
                })
            })
            .collect();
        for h in handles {
            // A worker only panics if `run` panics, which it does not for
            // any input; propagating would lose the block, so re-raise.
            let (i, traces) = h.join().unwrap_or_else(|e| std::panic::resume_unwind(e));
            out.insert(i, traces);
        }
    });
    out.into_values().flatten().collect()
}
