//! The reference: one transaction at a time, in block order.

use crate::exec::run;
use crate::{BlockResult, State, Tx};

/// Executes `txs` in order over `base`.
pub fn execute(base: &State, txs: &[Tx], work: u32) -> BlockResult {
    let mut writes = State::new();
    let mut receipts = Vec::with_capacity(txs.len());
    for tx in txs {
        let t = run(tx, work, |k| {
            writes
                .get(&k)
                .or_else(|| base.get(&k))
                .copied()
                .unwrap_or(0)
        });
        for (k, v) in &t.writes {
            writes.insert(*k, *v);
        }
        receipts.push(t.receipt);
    }
    BlockResult { writes, receipts }
}
