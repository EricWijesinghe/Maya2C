//! Declared access lists: conflict-free waves, each run in parallel.
//!
//! Transaction `i` joins wave `1 + max(wave(j))` over every earlier `j` it
//! conflicts with (write-write, write-read, read-write on a declared key).
//! Within a wave no two transactions touch a key one of them writes, so they
//! commute, and running waves in order is equivalent to block order. The
//! assignment is a pure function of the declarations: deterministic.
//!
//! A transaction that touches an undeclared key is **failed** (its writes
//! discarded, base gas charged) — in both this scheduler and the reference
//! used to compare against it — so a lying declaration costs its sender and
//! can never make two nodes disagree.

use std::collections::BTreeMap;

use crate::exec::{BASE_GAS, Trace, run};
use crate::{BlockResult, Key, Receipt, State, Stats, Tx};

/// Wave index per transaction.
pub fn assign(txs: &[Tx]) -> Vec<usize> {
    let mut last_write: BTreeMap<Key, usize> = BTreeMap::new();
    let mut last_read: BTreeMap<Key, usize> = BTreeMap::new();
    let mut waves = Vec::with_capacity(txs.len());
    for tx in txs {
        let mut w = 0usize;
        for k in &tx.reads {
            if let Some(x) = last_write.get(k) {
                w = w.max(x + 1);
            }
        }
        for k in &tx.writes {
            if let Some(x) = last_write.get(k) {
                w = w.max(x + 1);
            }
            if let Some(x) = last_read.get(k) {
                w = w.max(x + 1);
            }
        }
        for k in &tx.reads {
            let e = last_read.entry(*k).or_insert(w);
            *e = (*e).max(w);
        }
        for k in &tx.writes {
            last_write.insert(*k, w);
        }
        waves.push(w);
    }
    waves
}

fn enforce_declarations(mut t: Trace) -> Trace {
    if t.undeclared {
        t.writes.clear();
        t.receipt = Receipt {
            success: false,
            gas: BASE_GAS,
        };
    }
    t
}

/// Executes `txs` wave by wave with `threads` workers per wave.
pub fn execute(base: &State, txs: &[Tx], threads: usize, work: u32) -> (BlockResult, Stats) {
    let wave_of = assign(txs);
    let n_waves = wave_of.iter().copied().max().map_or(0, |m| m + 1);
    let mut committed = State::new();
    let mut traces: Vec<Option<Trace>> = vec![None; txs.len()];
    for wave in 0..n_waves {
        let members: Vec<usize> = (0..txs.len()).filter(|i| wave_of[*i] == wave).collect();
        let view = &committed;
        let read = |k: Key| view.get(&k).or_else(|| base.get(&k)).copied().unwrap_or(0);
        let results: Vec<(usize, Trace)> = if threads <= 1 || members.len() < 2 * threads {
            members
                .iter()
                .map(|&i| (i, enforce_declarations(run(&txs[i], work, read))))
                .collect()
        } else {
            let chunk = members.len().div_ceil(threads);
            std::thread::scope(|s| {
                let handles: Vec<_> = members
                    .chunks(chunk)
                    .map(|part| {
                        s.spawn(move || {
                            part.iter()
                                .map(|&i| (i, enforce_declarations(run(&txs[i], work, read))))
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|h| h.join().unwrap_or_else(|e| std::panic::resume_unwind(e)))
                    .collect()
            })
        };
        // Commit in block order within the wave (they commute; the order only
        // makes the map writes deterministic to read).
        let mut results = results;
        results.sort_by_key(|(i, _)| *i);
        for (i, t) in results {
            for (k, v) in &t.writes {
                committed.insert(*k, *v);
            }
            traces[i] = Some(t);
        }
    }
    let receipts = traces
        .into_iter()
        .map(|t| t.map(|t| t.receipt).unwrap_or_default())
        .collect();
    (
        BlockResult {
            writes: committed,
            receipts,
        },
        Stats {
            reexecuted: 0,
            waves: n_waves,
        },
    )
}

/// The sequential reference under the same declaration rule.
pub fn sequential_with_declarations(base: &State, txs: &[Tx], work: u32) -> BlockResult {
    let mut writes = State::new();
    let mut receipts = Vec::with_capacity(txs.len());
    for tx in txs {
        let t = enforce_declarations(run(tx, work, |k| {
            writes
                .get(&k)
                .or_else(|| base.get(&k))
                .copied()
                .unwrap_or(0)
        }));
        for (k, v) in &t.writes {
            writes.insert(*k, *v);
        }
        receipts.push(t.receipt);
    }
    BlockResult { writes, receipts }
}
