//! Dark pool batch of 1,000 orders: commit, reveal, clear. Prints measured
//! timings; no target is asserted.

#![allow(clippy::cast_possible_truncation)]

use std::time::Instant;

use maya_permissioned_finance::darkpool::{self, Batch, Order, Side};

const ORDERS: usize = 1_000;
const RUNS: usize = 20;

fn orders() -> Vec<(Order, [u8; 32])> {
    (0..ORDERS)
        .map(|i| {
            let mut trader = [0u8; 32];
            trader[..8].copy_from_slice(&(i as u64).to_le_bytes());
            let order = Order {
                side: if i % 2 == 0 { Side::Buy } else { Side::Sell },
                price: 90 + (i as u64 * 7919) % 21,
                size: 1 + (i as u64 * 104_729) % 50,
                trader,
            };
            (order, [(i % 256) as u8; 32])
        })
        .collect()
}

fn main() {
    let input = orders();
    let mut best = [f64::MAX; 3];
    let mut price = None;
    for _ in 0..RUNS {
        let t = Instant::now();
        let mut batch = Batch::new();
        let slots: Vec<usize> = input
            .iter()
            .map(|(o, s)| batch.place(darkpool::commit(o, s)))
            .collect();
        let commit_ms = t.elapsed().as_secs_f64() * 1e3;

        let t = Instant::now();
        for (slot, (o, s)) in slots.iter().zip(&input) {
            assert!(batch.reveal(*slot, *o, s));
        }
        let reveal_ms = t.elapsed().as_secs_f64() * 1e3;

        let t = Instant::now();
        let clearing = batch.close();
        let clear_ms = t.elapsed().as_secs_f64() * 1e3;
        price = clearing.price;
        for (b, v) in best.iter_mut().zip([commit_ms, reveal_ms, clear_ms]) {
            *b = b.min(v);
        }
    }
    println!(
        "darkpool {ORDERS} orders (best of {RUNS}): commit {:.3} ms, reveal {:.3} ms, clear {:.3} ms, total {:.3} ms; price {price:?}",
        best[0],
        best[1],
        best[2],
        best.iter().sum::<f64>()
    );
}
