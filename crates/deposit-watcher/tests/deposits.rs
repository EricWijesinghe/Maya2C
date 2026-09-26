//! 100,000 deposits with the service killed (SIGKILL) at random points
//! (Master Prompt 17 §1): zero double credits, zero missed credits.
//!
//! The test binary re-executes itself as a child (`MAYA_DEPOSIT_CHILD=<dir>`)
//! that runs the watcher against a deterministic synthetic chain. The parent
//! kills it after a seeded random delay, repeatedly, then lets one run finish
//! and compares the journal-derived ledger with the chain's ground truth.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::cast_possible_truncation
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use maya_deposit_watcher::{BlockSource, Transfer, Watcher};

const BLOCKS: u64 = 10_010;
const PER_BLOCK: u64 = 12;
const CUSTOMERS: u64 = 500;

/// Block `h` carries `PER_BLOCK` transfers; 10 of every 12 are deposits to a
/// customer address, 2 go elsewhere. One transfer per 1,000 blocks is a
/// re-delivery of an earlier txid, which must not be credited twice.
struct Chain;

fn customer(i: u64) -> String {
    format!("{:064x}", i + 1)
}

impl BlockSource for Chain {
    fn finalized_height(&self) -> u64 {
        BLOCKS
    }
    fn transfers(&self, h: u64) -> Result<Vec<Transfer>, String> {
        Ok((0..PER_BLOCK)
            .map(|i| {
                let n = h * PER_BLOCK + i;
                let redelivery = h > 1 && i == 0 && h.is_multiple_of(1_000);
                let txid = if redelivery {
                    format!("tx{}", n - PER_BLOCK)
                } else {
                    format!("tx{n}")
                };
                let to = if i % 6 == 5 {
                    format!("{:064x}", u64::MAX - n)
                } else {
                    customer(n % CUSTOMERS)
                };
                Transfer {
                    txid,
                    to,
                    amount: 1 + n % 97,
                }
            })
            .collect())
    }
}

fn watched() -> BTreeSet<String> {
    (0..CUSTOMERS).map(customer).collect()
}

fn ground_truth() -> (BTreeMap<String, u128>, usize) {
    let (mut balances, mut seen) = (BTreeMap::new(), BTreeSet::new());
    let w = watched();
    for h in 1..=BLOCKS {
        for t in Chain.transfers(h).unwrap() {
            if w.contains(&t.to) && seen.insert(t.txid.clone()) {
                *balances.entry(t.to).or_insert(0u128) += u128::from(t.amount);
            }
        }
    }
    (balances, seen.len())
}

#[test]
fn deposit_child() {
    let Ok(dir) = std::env::var("MAYA_DEPOSIT_CHILD") else {
        return;
    };
    let mut w = Watcher::open(&Path::new(&dir).join("journal.jsonl"), watched()).unwrap();
    w.poll(&Chain).unwrap();
}

#[test]
fn a_hundred_thousand_deposits_across_kill_nine_restarts_credit_exactly_once() {
    let dir = tempfile::TempDir::new().unwrap();
    let exe = std::env::current_exe().unwrap();
    let journal = dir.path().join("journal.jsonl");
    let mut seed = 0xD0_5EED_u64;
    let (mut kills, mut last_cursor) = (0, 0);
    for _ in 0..40 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let mut child = Command::new(&exe)
            .args(["deposit_child", "--exact", "--test-threads", "1"])
            .env("MAYA_DEPOSIT_CHILD", dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(30 + seed % 120));
        if child.try_wait().unwrap().is_none() {
            child.kill().unwrap();
            kills += 1;
        }
        child.wait().unwrap();
        let cursor = Watcher::open(&journal, watched()).unwrap().cursor();
        assert!(cursor >= last_cursor, "the cursor went backwards");
        last_cursor = cursor;
        if cursor == BLOCKS {
            break;
        }
    }
    let mut w = Watcher::open(&journal, watched()).unwrap();
    w.poll(&Chain).unwrap();
    let (truth, deposits) = ground_truth();
    println!(
        "{deposits} deposits over {BLOCKS} blocks; {kills} SIGKILLs mid-run; credited {}; ledger matches ground truth: {}",
        w.credited(),
        w.balances() == &truth
    );
    assert!(
        deposits >= 100_000,
        "the scenario must carry at least 100,000 deposits"
    );
    assert!(kills > 0, "at least one run must have been killed mid-way");
    assert_eq!(w.credited(), deposits, "missed or double-credited deposits");
    assert_eq!(w.balances(), &truth);
}

/// The first three blocks of [`Chain`].
struct Short;

impl BlockSource for Short {
    fn finalized_height(&self) -> u64 {
        3
    }
    fn transfers(&self, h: u64) -> Result<Vec<Transfer>, String> {
        Chain.transfers(h)
    }
}

#[test]
fn a_torn_final_line_is_discarded_and_the_block_reprocessed() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("j.jsonl");
    let mut w = Watcher::open(&path, watched()).unwrap();
    w.poll(&Short).unwrap();
    let full = w.balances().clone();
    drop(w);
    // Simulate a crash mid-write of block 3's line.
    let text = std::fs::read_to_string(&path).unwrap();
    let cut = text.trim_end().rfind('\n').unwrap() + 1 + 10;
    std::fs::write(&path, &text[..cut]).unwrap();
    let mut w = Watcher::open(&path, watched()).unwrap();
    assert_eq!(w.cursor(), 2);
    w.poll(&Short).unwrap();
    assert_eq!(w.balances(), &full);
}
