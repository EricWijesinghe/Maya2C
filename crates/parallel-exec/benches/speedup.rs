//! Speedup curves (Master Prompt 12 §1): 1-32 threads at 0/10/50/90%
//! contention, for both schedulers, against sequential execution.
//!
//! Each transaction carries `WORK` iterations of an integer mix as a stand-in
//! for signature/VM cost (stated in the output). The machine's core count is
//! printed, because a speedup curve past the number of cores measures the
//! scheduler, not parallelism. `cargo bench -p maya-parallel-exec --bench speedup`.

#![allow(clippy::unwrap_used, clippy::cast_precision_loss)]

use std::time::Instant;

use maya_loadgen::{Params, Workload};
use maya_parallel_exec::{State, optimistic, sequential, waves};

const TXS: usize = 10_000;
const WORK: u32 = 2_000;
const REPS: usize = 3;

fn best_ms(mut f: impl FnMut()) -> f64 {
    (0..REPS)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1e3
        })
        .fold(f64::MAX, f64::min)
}

fn main() {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    println!("parallel-exec speedup: {TXS} txs/block, {WORK} mix iterations/tx of synthetic work, {cores} cores available");
    println!("{:>10} {:>7} {:>12} {:>9} {:>11} {:>9} {:>7}", "contention", "threads", "scheduler", "ms", "tx/s", "speedup", "redo");
    for contention in [0u32, 100_000, 500_000, 900_000] {
        let txs = Workload::new(Params { contention_ppm: contention, accounts: 100_000, ..Params::default() }).take(TXS);
        let mut base = State::new();
        for a in 0..100_000 {
            base.insert(a, 1_000_000);
        }
        let seq = best_ms(|| {
            std::hint::black_box(sequential::execute(&base, &txs, WORK));
        });
        println!("{:>9}% {:>7} {:>12} {:>9.1} {:>11.0} {:>9.2} {:>7}", contention / 10_000, 1, "sequential", seq, TXS as f64 / seq * 1e3, 1.0, "-");
        for threads in [1usize, 2, 4, 8, 16, 32] {
            let mut redo = 0;
            let opt = best_ms(|| {
                let (r, s) = optimistic::execute(&base, &txs, threads, WORK);
                redo = s.reexecuted;
                std::hint::black_box(r);
            });
            let mut n_waves = 0;
            let wv = best_ms(|| {
                let (r, s) = waves::execute(&base, &txs, threads, WORK);
                n_waves = s.waves;
                std::hint::black_box(r);
            });
            println!("{:>9}% {:>7} {:>12} {:>9.1} {:>11.0} {:>9.2} {:>7}", contention / 10_000, threads, "optimistic", opt, TXS as f64 / opt * 1e3, seq / opt, redo);
            println!("{:>9}% {:>7} {:>12} {:>9.1} {:>11.0} {:>9.2} {:>7}", contention / 10_000, threads, "waves", wv, TXS as f64 / wv * 1e3, seq / wv, format!("{n_waves}w"));
        }
    }
}
