//! `kill -9` during commit, 1,000 times nightly (Master Prompt 12 §3).
//!
//! The test binary re-executes itself as a child (`MAYA_CRASH_CHILD=<dir>`)
//! that opens the chain on `dir` and commits blocks as fast as it can. Once
//! the child reports it is committing, the parent SIGKILLs it after a seeded
//! random delay, reopens the database, and
//! checks that it is consistent: the chain opens, the committed state root
//! equals the tip header's `state_root`, and the height never goes backwards
//! (a committed block is never lost to a process crash).
//!
//! What this covers: process death at any instruction, including mid
//! `WriteBatch`. What it does not: power loss, where unsynced OS buffers are
//! lost too — the `RocksDB` WAL is not fsynced per write here, which is a stated
//! storage choice (ADR-019), not an oversight.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::state::db::StateDB;

fn genesis() -> Block {
    Block::new(
        BlockHeader {
            prev_hash: [0u8; 32],
            state_root: [0u8; 32],
            timestamp: 1_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

fn open(dir: &Path) -> Chain {
    let state = Arc::new(StateDB::open(dir).expect("open state"));
    Chain::open(state, genesis(), ChainConfig::without_pow_verification()).expect("open chain")
}

/// Child mode: commit blocks until killed. A no-op when run as a normal test.
#[test]
fn crash_child() {
    let Ok(dir) = std::env::var("MAYA_CRASH_CHILD") else {
        return;
    };
    let mut chain = open(Path::new(&dir));
    // The parent times its kill from this line, not from spawn: under load,
    // process start and opening the database can outlast the whole random
    // delay, and a kill before the first commit tests startup, not commit.
    println!("{READY}");
    std::io::Write::flush(&mut std::io::stdout()).expect("flush");
    loop {
        let timestamp = 1_000_000 + (chain.height() + 1) * 15;
        let block = chain
            .candidate_block(timestamp, Vec::new())
            .expect("candidate");
        chain.insert_block(block).expect("insert");
    }
}

/// What the child prints once it is committing.
const READY: &str = "maya-crash-child committing";

/// Blocks until the child prints [`READY`], or its stdout closes.
fn wait_ready(child: &mut std::process::Child) {
    use std::io::BufRead as _;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = std::io::BufReader::new(stdout).lines();
    while let Some(Ok(line)) = lines.next() {
        if line.contains(READY) {
            // Keep draining so the child never blocks on a full pipe.
            std::thread::spawn(move || lines.for_each(drop));
            return;
        }
    }
}

fn check(dir: &Path) -> u64 {
    let chain = open(dir);
    let tip = chain.get(&chain.tip()).expect("tip record");
    let root = chain.state().state_root().expect("root");
    if chain.height() > 0 {
        assert_eq!(
            root, tip.header.state_root,
            "state root disagrees with the tip header"
        );
    }
    chain.height()
}

#[test]
fn kill_nine_during_commit_always_restarts_consistent() {
    let runs: u32 = std::env::var("CRASH_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        // 250 per pull request; nightly runs the brief's 1,000 through
        // CRASH_RUNS. Each run waits for the child to be committing, about
        // 0.8 s here, so 1,000 is 14 minutes on its own.
        .unwrap_or(250);
    let dir = tempfile::TempDir::new().expect("dir");
    let exe = std::env::current_exe().expect("test binary");
    let mut height = check(dir.path());
    let mut seed = 0x6B11_u64;
    let mut advanced = 0u32;
    for run in 0..runs {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let delay = Duration::from_millis(20 + seed % 80);
        let mut child = Command::new(&exe)
            .args([
                "crash_child",
                "--exact",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env("MAYA_CRASH_CHILD", dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn child");
        wait_ready(&mut child);
        std::thread::sleep(delay);
        child.kill().expect("SIGKILL"); // SIGKILL on Unix: no destructors, no flush
        child.wait().expect("reap");
        let now = check(dir.path());
        assert!(
            now >= height,
            "run {run} (seed {seed:#x}): height went back from {height} to {now}"
        );
        if now > height {
            advanced += 1;
        }
        height = now;
    }
    println!(
        "{runs} kill -9 runs: every restart consistent; final height {height}; {advanced} runs committed at least one block"
    );
    assert!(
        advanced > runs / 4,
        "most runs should have committed something"
    );
}
