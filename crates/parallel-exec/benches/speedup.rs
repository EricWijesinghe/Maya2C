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
/// Ten times the transfer stand-in: roughly a small contract call.
const HEAVY: u32 = 20_000;
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

fn row(label: &str, threads: usize, scheduler: &str, ms: f64, seq: f64, redo: &str) {
    println!(
        "{label:>18} {threads:>7} {scheduler:>12} {ms:>9.1} {:>11.0} {:>9.2} {redo:>7}",
        TXS as f64 / ms * 1e3,
        seq / ms
    );
}

/// One curve: sequential, then both schedulers at every thread count.
fn curve(label: &str, params: Params, work: u32) {
    let txs = Workload::new(params).take(TXS);
    let mut base = State::new();
    for a in 0..params.accounts {
        base.insert(a, 1_000_000);
    }
    let seq = best_ms(|| {
        std::hint::black_box(sequential::execute(&base, &txs, work));
    });
    row(label, 1, "sequential", seq, seq, "-");
    for threads in [1usize, 2, 4, 8, 16, 32] {
        let mut redo = 0;
        let opt = best_ms(|| {
            let (r, s) = optimistic::execute(&base, &txs, threads, work);
            redo = s.reexecuted;
            std::hint::black_box(r);
        });
        let mut n_waves = 0;
        let wv = best_ms(|| {
            let (r, s) = waves::execute(&base, &txs, threads, work);
            n_waves = s.waves;
            std::hint::black_box(r);
        });
        row(label, threads, "optimistic", opt, seq, &redo.to_string());
        row(label, threads, "waves", wv, seq, &format!("{n_waves}w"));
    }
}

fn main() {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    println!(
        "parallel-exec speedup: {TXS} txs/block, synthetic work per tx as stated per curve, {cores} cores available"
    );
    println!(
        "{:>18} {:>7} {:>12} {:>9} {:>11} {:>9} {:>7}",
        "curve", "threads", "scheduler", "ms", "tx/s", "speedup", "redo"
    );
    // The brief's four contention levels, over the default Zipf(1.0) account
    // distribution — which is itself a source of conflicts, so "0%" here means
    // no *hot-key* share, not no conflicts.
    for contention in [0u32, 100_000, 500_000, 900_000] {
        let label = format!("zipf1 hot{}% w{WORK}", contention / 10_000);
        curve(
            &label,
            Params {
                contention_ppm: contention,
                accounts: 100_000,
                ..Params::default()
            },
            WORK,
        );
    }
    // Controls: uniform accounts and no hot keys isolate the scheduler's own
    // overhead; the heavier work isolates granularity (a contract call, not a
    // transfer).
    let uniform = Params {
        contention_ppm: 0,
        zipf_s_x100: 0,
        accounts: 100_000,
        ..Params::default()
    };
    curve(&format!("uniform w{WORK}"), uniform, WORK);
    curve(&format!("uniform w{HEAVY}"), uniform, HEAVY);
    curve(
        &format!("zipf1 hot50% w{HEAVY}"),
        Params {
            contention_ppm: 500_000,
            zipf_s_x100: 100,
            ..uniform
        },
        HEAVY,
    );
}
