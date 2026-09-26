//! The transaction semantics, as a pure function of the values read.
//!
//! [`run`] takes a read function and returns the reads it made (key, value)
//! and the writes it wants. Because the output is a function of the read
//! *values* only, a transaction whose reads are unchanged would produce the
//! same writes and gas — which is what makes value-based validation sound.

use maya_loadgen::{HOT_BASE, Kind, Tx};

use crate::{Key, Receipt};

/// Base gas per transaction.
pub const BASE_GAS: u64 = 21_000;
/// Gas per key read.
pub const READ_GAS: u64 = 100;
/// Gas per key written.
pub const WRITE_GAS: u64 = 200;

/// One execution's trace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trace {
    /// Every read, in order, with the value seen.
    pub reads: Vec<(Key, u64)>,
    /// Final writes.
    pub writes: Vec<(Key, u64)>,
    /// Receipt.
    pub receipt: Receipt,
    /// Whether the transaction touched a key it did not declare.
    pub undeclared: bool,
}

/// Synthetic per-transaction work: `iterations` rounds of an integer mix,
/// standing in for signature and VM cost. The result is folded into nothing
/// observable except through `black_box`, so it cannot change semantics.
pub fn burn(iterations: u32, seed: u64) {
    let mut x = seed | 1;
    for _ in 0..iterations {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
    }
    std::hint::black_box(x);
}

/// Executes `tx`, reading through `read`.
pub fn run(tx: &Tx, work: u32, mut read: impl FnMut(Key) -> u64) -> Trace {
    burn(work, tx.id);
    let mut t = Trace::default();
    let mut get = |k: Key, t: &mut Trace| {
        let v = read(k);
        if !tx.reads.contains(&k) {
            t.undeclared = true;
        }
        t.reads.push((k, v));
        v
    };
    let hot = tx
        .writes
        .iter()
        .copied()
        .find(|k| (HOT_BASE..maya_loadgen::CONTRACT_BASE).contains(k));
    let success = match tx.kind {
        Kind::Transfer | Kind::TokenTransfer => {
            // Operands come from the write set; the read set is only the
            // declaration, which may lie (and is then caught as undeclared).
            let (from, to) = match tx.kind {
                Kind::Transfer => (tx.writes[0], tx.writes[1]),
                _ => (tx.writes[1], tx.writes[2]),
            };
            let fb = get(from, &mut t);
            let tb = get(to, &mut t);
            match (fb.checked_sub(tx.amount), tb.checked_add(tx.amount)) {
                (Some(f), Some(to_b)) => {
                    t.writes.push((from, f));
                    t.writes.push((to, to_b));
                    true
                }
                _ => false,
            }
        }
        Kind::ContractCall => {
            let slot = tx.writes[1];
            let v = get(slot, &mut t);
            t.writes.push((slot, v.wrapping_add(1)));
            true
        }
        Kind::Deploy => {
            let code = tx.writes[1];
            t.writes.push((code, tx.id));
            true
        }
    };
    if success && let Some(h) = hot {
        let v = get(h, &mut t);
        t.writes.push((h, v.wrapping_add(tx.amount)));
    }
    let writes = t.writes.len() as u64;
    let reads = t.reads.len() as u64;
    t.receipt = Receipt {
        success,
        gas: BASE_GAS + READ_GAS * reads + WRITE_GAS * writes,
    };
    t
}
