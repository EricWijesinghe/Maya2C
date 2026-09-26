//! Parallel == sequential, byte for byte, on 100,000 random blocks
//! (Master Prompt 12 DONE WHEN). Any mismatch prints its seed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation
)]

use maya_loadgen::{Params, Workload};
use maya_parallel_exec::{State, optimistic, sequential, waves};

const BLOCKS: u64 = 100_000;

fn splitmix(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[test]
fn parallel_equals_sequential_on_100k_random_blocks() {
    let mut rng = 0xD1FF_u64;
    let mut reexecuted = 0usize;
    let mut total_txs = 0usize;
    for block in 0..BLOCKS {
        let seed = splitmix(&mut rng);
        let contention = [0u32, 100_000, 500_000, 900_000][(seed % 4) as usize];
        let params = Params {
            seed,
            accounts: 8 + seed % 64,
            hot_keys: 1 + seed % 4,
            contention_ppm: contention,
            zipf_s_x100: (seed % 150) as u32,
            ..Params::default()
        };
        let n = 1 + (seed >> 8) as usize % 48;
        let mut txs = Workload::new(params).take(n);
        // Some senders are broke, so transfers fail and later ones depend on
        // earlier successes.
        let mut base = State::new();
        for a in 0..params.accounts {
            if !splitmix(&mut rng).is_multiple_of(4) {
                base.insert(a, splitmix(&mut rng) % 3_000);
            }
        }
        // One block in eight carries a lying declaration.
        if seed.is_multiple_of(8) && txs[0].reads.len() > 1 {
            txs[0].reads.pop();
        }
        let threads = 1 + (seed >> 16) as usize % 8;
        let reference = sequential::execute(&base, &txs, 0);
        let (opt, stats) = optimistic::execute(&base, &txs, threads, 0);
        assert_eq!(
            opt, reference,
            "optimistic diverged: block {block} seed {seed:#x} threads {threads}"
        );
        let declared_ref = waves::sequential_with_declarations(&base, &txs, 0);
        let (wv, _) = waves::execute(&base, &txs, threads, 0);
        assert_eq!(
            wv, declared_ref,
            "waves diverged: block {block} seed {seed:#x} threads {threads}"
        );
        reexecuted += stats.reexecuted;
        total_txs += n;
    }
    println!(
        "{BLOCKS} blocks, {total_txs} txs, {reexecuted} optimistic re-executions: parallel == sequential every time"
    );
}

#[test]
fn gas_does_not_depend_on_thread_count() {
    let txs = Workload::new(Params {
        contention_ppm: 500_000,
        accounts: 32,
        ..Params::default()
    })
    .take(2_000);
    let mut base = State::new();
    for a in 0..32 {
        base.insert(a, 10_000);
    }
    let gas = |threads| optimistic::execute(&base, &txs, threads, 0).0.receipts;
    let one = gas(1);
    for t in [2, 4, 8, 16, 32] {
        assert_eq!(gas(t), one, "{t} threads");
    }
}
