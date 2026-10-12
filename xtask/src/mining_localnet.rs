//! `cargo xtask mining-localnet [--gpu] [--seconds N]` — ADR-044's block
//! rewards on four validators, mined by the release `maya2c-gpu-miner`.
//!
//! The localnet's four processes, with fees, a funded reserve and `bft.mining`
//! in genesis, and the miner pointed at node 1 (so shares cross the p2p layer
//! before node 0 sees them). It checks, and exits non-zero on the first
//! failure:
//!
//! 1. a share pays once mining opens at the first epoch boundary;
//! 2. with validator 3 killed (f = 1 of n = 4), shares keep paying;
//! 3. at the end, all four nodes agree on one chain and report the same
//!    miner balance and the same reserve, and nothing was minted: the reserve
//!    fell by at least what the miner holds.
//!
//! One machine, localhost links, a 4 MiB test dataset: it proves the binaries
//! agree on mining as a deployment would run them, not how a network of
//! strangers behaves.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::devnet::{self, Procs, Result};
use crate::localnet::{self, VALIDATORS};

/// `MINING_RESERVE`: "maya2c/mining/reserve/v1" zero-padded to 32 bytes. A
/// wrong value fails loudly, because genesis refuses an unfunded reserve.
const RESERVE_ADDRESS: &str = "6d61796132632f6d696e696e672f726573657276652f76310000000000000000";
const RESERVE: u64 = 1_000_000_000;
const MINER_NODE: u16 = 1;
const DEFAULT_SECONDS: u64 = 120;
const PORT_OFFSET: u16 = 2_000;

/// Fees, the reserve and `bft.mining`: what a mining chain adds to the
/// localnet genesis. Easy target and short epochs so a run crosses several.
fn mining_genesis(genesis: &mut Value) {
    if let Some(allocations) = genesis["allocations"].as_array_mut() {
        allocations.push(json!({ "address": RESERVE_ADDRESS, "balance": RESERVE }));
    }
    genesis["bft"]["fees"] = json!({
        "initial_base_fee": 1, "min_base_fee": 1,
        "target_block_bytes": 2_621_440, "change_denominator": 8
    });
    genesis["bft"]["mining"] = json!({
        "initial_epoch_reward": 1_000_000, "halving_epochs": 100,
        "miner_share_bps": 7_000, "target_shares": 10, "max_shares_per_block": 2,
        "initial_target": format!("00{}", "ff".repeat(31)), "dag": "testing"
    });
}

fn build_miner(gpu: bool) -> Result<PathBuf> {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.args(["build", "--release", "-p", "maya2c-gpu-miner"])
        .current_dir(devnet::root())
        .stdout(Stdio::null());
    if gpu {
        cmd.args(["--features", "gpu"]);
    }
    let status = cmd
        .status()
        .map_err(|e| format!("cargo build miner: {e}"))?;
    if !status.success() {
        return Err(format!("cargo build miner failed: {status}"));
    }
    Ok(devnet::root()
        .join("target/release")
        .join(format!("maya2c-gpu-miner{}", std::env::consts::EXE_SUFFIX)))
}

/// The miner's events so far, one JSON value per line.
fn events(log: &Path) -> Vec<Value> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn miner_address(log: &Path) -> Result<String> {
    let mut found = None;
    localnet::wait_until("the miner's start event", || {
        found = events(log)
            .iter()
            .find(|e| e["event"] == "start")
            .and_then(|e| e["address"].as_str().map(str::to_owned));
        found.is_some()
    })?;
    found.ok_or_else(|| "no start event".into())
}

fn paid(i: u16, address: &str) -> u64 {
    localnet::balance(i, address).unwrap_or(0)
}

/// Runs the rehearsal in `target/mining-localnet`.
///
/// # Errors
///
/// The first check that fails, with node and miner logs left in place.
pub fn mining_localnet(args: &[String]) -> Result<()> {
    let gpu = args.iter().any(|a| a == "--gpu");
    let seconds = args
        .iter()
        .position(|a| a == "--seconds")
        .and_then(|i| args.get(i + 1))
        .map_or(Ok(DEFAULT_SECONDS), |s| s.parse::<u64>())
        .map_err(|e| format!("--seconds: {e}"))?;
    let work = devnet::root().join("target").join("mining-localnet");
    run(&work, gpu, seconds).map_err(|e| format!("{e} (logs: {})", work.display()))
}

fn run(work: &Path, gpu: bool, seconds: u64) -> Result<()> {
    // Clear of the live testnet on the owner's PC (seed p2p 31100, a
    // foundation validator's rpc 32000): p2p 33100+, rpc 34000+.
    localnet::use_port_offset(PORT_OFFSET);
    localnet::ports_free()?;
    devnet::build(&["maya2c-node", "l1-wallet"])?;
    let miner_bin = build_miner(gpu)?;
    let funder = localnet::setup_tweaked(work, &mining_genesis)?;
    let started = Instant::now();
    let mut nodes = Procs::default();
    for i in 0..VALIDATORS {
        nodes.push(localnet::start(work, i)?);
    }
    let all: Vec<u16> = (0..VALIDATORS).collect();
    localnet::wait_until("four nodes on one chain", || {
        localnet::agree(&all, &funder, 3)
    })?;
    println!("mining-localnet: {VALIDATORS} validators on one chain");

    let log = work.join("miner.jsonl");
    let mut miner = Procs::default();
    let mut cmd = Command::new(&miner_bin);
    cmd.args(["--mine", "--json", "--threads", "4", "--rpc"])
        .arg(format!("http://{}", localnet::rpc_addr(MINER_NODE)))
        .arg("--key-file")
        .arg(work.join("miner.key"))
        .stdout(std::fs::File::create(&log).map_err(|e| e.to_string())?)
        .stderr(Stdio::null());
    if gpu {
        cmd.arg("--gpu");
    }
    miner.push(cmd.spawn().map_err(|e| format!("starting miner: {e}"))?);
    let address = miner_address(&log)?;

    let t = localnet::wait_until("the first paid share", || paid(0, &address) > 0)?;
    println!("mining-localnet: first share paid {t:?} after the miner started");

    nodes.kill(usize::from(VALIDATORS - 1));
    let before = paid(0, &address);
    let t = localnet::wait_until("shares paying with one validator down", || {
        paid(0, &address) > before
    })?;
    println!("mining-localnet: validator 3 down; shares kept paying ({t:?})");
    nodes.replace(
        usize::from(VALIDATORS - 1),
        localnet::start(work, VALIDATORS - 1)?,
    );

    let remaining = Duration::from_secs(seconds).saturating_sub(started.elapsed());
    std::thread::sleep(remaining);
    drop(miner);
    report(&all, &funder, &address, &log)?;
    println!("mining-localnet: PASS in {:?}", started.elapsed());
    Ok(())
}

/// The end-of-run agreement checks, and what the run measured.
fn report(all: &[u16], funder: &str, address: &str, log: &Path) -> Result<()> {
    // Settle: wait for every node, the restarted one included, to share a tip.
    let (height, _) = localnet::tip(0, funder)?;
    localnet::wait_until("all four nodes agreeing at the end", || {
        localnet::agree(all, funder, height)
    })?;
    let miner: Vec<u64> = all.iter().map(|i| paid(*i, address)).collect();
    let reserve: Vec<u64> = all.iter().map(|i| paid(*i, RESERVE_ADDRESS)).collect();
    if miner.iter().any(|b| *b != miner[0]) || reserve.iter().any(|b| *b != reserve[0]) {
        return Err(format!(
            "nodes disagree: miner {miner:?}, reserve {reserve:?}"
        ));
    }
    if RESERVE - reserve[0] < miner[0] {
        return Err(format!(
            "minted: the miner holds {} but the reserve fell only {}",
            miner[0],
            RESERVE - reserve[0]
        ));
    }
    let events = events(log);
    let accepted = events.iter().filter(|e| e["accepted"] == true).count();
    let refused = events.iter().filter(|e| e["accepted"] == false).count();
    let rate = events
        .iter()
        .rev()
        .find(|e| e["event"] == "hashrate")
        .and_then(|e| e["hps"].as_f64())
        .unwrap_or(0.0);
    let targets: Vec<String> = events
        .iter()
        .filter(|e| e["event"] == "work")
        .filter_map(|e| {
            Some(format!(
                "{}:{}",
                e["epoch"],
                e["target"].as_str()?.get(..8)?
            ))
        })
        .collect();
    println!(
        "mining-localnet: miner balance {} on all four nodes",
        miner[0]
    );
    println!("mining-localnet: reserve paid out {}", RESERVE - reserve[0]);
    println!("mining-localnet: shares sent {accepted}, refused {refused}, last rate {rate:.0} H/s");
    println!("mining-localnet: targets by epoch {}", targets.join(" "));
    Ok(())
}
