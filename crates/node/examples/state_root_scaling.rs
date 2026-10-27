//! How the cost of `StateDB::state_root` grows with the number of accounts
//! (ADR-062, which says a state-root format change must be measured first).
//!
//! The dense accounts tree re-reads every account and re-hashes every leaf each
//! time a root is computed, and a block's execution computes one. This prints
//! the time per root at several account counts, so "O(all accounts) per block"
//! is a measured curve rather than a reading of the code.
//!
//! What it is and is not:
//! - **Is:** the whole `state_root()` call on a warm RocksDB, median of the runs.
//! - **Is not:** a block-apply time (signatures, execution, the write batch are
//!   not in it), and not the sparse tree: that path recomputes from the same
//!   merged map, so only an incremental update, which is not built, would
//!   change the curve.
//!
//! ```text
//! cargo run --profile perf -p custom-l1-node --example state_root_scaling -- [runs] [counts...]
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::time::{Duration, Instant};

use custom_l1_node::state::account::Account;
use custom_l1_node::state::db::StateDB;
use primitive_types::U256;
use tempfile::TempDir;

const DEFAULT_COUNTS: [usize; 4] = [1_000, 10_000, 100_000, 300_000];
const DEFAULT_RUNS: usize = 5;

fn address(index: usize) -> [u8; 32] {
    let mut a = [0u8; 32];
    a[..8].copy_from_slice(&(index as u64).to_le_bytes());
    a[8..16].copy_from_slice(&(index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).to_le_bytes());
    a
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runs = args
        .first()
        .and_then(|a| a.parse().ok())
        .unwrap_or(DEFAULT_RUNS);
    let counts: Vec<usize> = if args.len() > 1 {
        args[1..].iter().filter_map(|a| a.parse().ok()).collect()
    } else {
        DEFAULT_COUNTS.to_vec()
    };

    println!("accounts,fill_s,root_ms_median,root_ms_min,root_ms_max,runs");
    for count in counts {
        let dir = TempDir::new().unwrap();
        let state = StateDB::open(dir.path()).unwrap();
        let fill = Instant::now();
        for i in 0..count {
            state
                .put_account(
                    &address(i),
                    &Account {
                        balance: U256::from(1_000 + i as u64),
                        nonce: i as u64 % 7,
                    },
                )
                .unwrap();
        }
        let fill = fill.elapsed();

        // One untimed root so the first run is not paying for a cold cache.
        let _ = state.state_root().unwrap();
        let mut times = Vec::with_capacity(runs);
        for _ in 0..runs {
            let t = Instant::now();
            let root = state.state_root().unwrap();
            times.push(t.elapsed());
            std::hint::black_box(root);
        }
        let ms = |d: Duration| d.as_secs_f64() * 1_000.0;
        println!(
            "{count},{:.2},{:.2},{:.2},{:.2},{runs}",
            fill.as_secs_f64(),
            ms(median(times.clone())),
            ms(*times.iter().min().unwrap()),
            ms(*times.iter().max().unwrap()),
        );
    }
}
