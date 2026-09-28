//! What one state root costs as the account set grows (Master Prompt 12).
//!
//! `StateDB::state_root` re-reads every account from `RocksDB` and hashes the
//! whole accounts tree on every call, and a block computes a root at least
//! once. This measures both halves at several sizes, so the case for an
//! incremental tree rests on a number.
//!
//! ```text
//! cargo run --profile perf -p custom-l1-node --example state_root_cost
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use std::time::Instant;

use custom_l1_node::state::{Account, StateDB};

const SIZES: &[u32] = &[2_000, 20_000, 200_000];
const REPEATS: u32 = 5;

fn main() {
    for &n in SIZES {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDB::open(dir.path()).unwrap();
        for i in 0..n {
            let mut address = [0u8; 32];
            address[..4].copy_from_slice(&i.to_le_bytes());
            address[4..8].copy_from_slice(&i.wrapping_mul(2_654_435_761).to_le_bytes());
            db.put_account(
                &address,
                &Account {
                    balance: u64::from(i) + 1,
                    nonce: 0,
                },
            )
            .unwrap();
        }
        let (mut scan, mut root) = (0.0, 0.0);
        for _ in 0..REPEATS {
            let t = Instant::now();
            let accounts = db.all_accounts().unwrap();
            scan += t.elapsed().as_secs_f64();
            assert_eq!(accounts.len(), n as usize);
            let t = Instant::now();
            let _ = db.state_root().unwrap();
            root += t.elapsed().as_secs_f64();
        }
        let (scan, root) = (
            scan / f64::from(REPEATS) * 1e3,
            root / f64::from(REPEATS) * 1e3,
        );
        println!(
            "{n:>7} accounts: scan {scan:8.2} ms, full root {root:8.2} ms ({:.0}% of it the scan), {:.2} µs/account",
            100.0 * scan / root,
            root * 1e3 / f64::from(n)
        );
    }
}
