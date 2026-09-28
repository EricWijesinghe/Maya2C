//! `cargo xtask localnet` — the deployment rehearsed on one machine: four
//! `maya2c-node` validators as separate processes, a genesis naming all four
//! as the DAG-BFT committee, real libp2p over TCP on 127.0.0.1.
//!
//! What it checks, in order, and exits non-zero on the first failure:
//!
//! 1. every node reaches the same block id at the same height — one chain,
//!    not four;
//! 2. a transfer sent to one node executes on all four;
//! 3. with one validator killed (f = 1 of n = 4), the other three keep
//!    committing blocks;
//! 4. restarted from its data directory, that validator catches up to the
//!    same block id as the rest.
//!
//! It proves the binaries, the genesis format and the p2p wiring work
//! together as a deployment would use them. It does not prove anything about
//! a network spread over real latency, hostile peers or other operators.

use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::devnet::{self, Procs, Result};

pub(crate) const VALIDATORS: u16 = 4;
const RPC_BASE: u16 = devnet::RPC_PORT;
const P2P_BASE: u16 = 31_100;
const GENESIS_BALANCE: u64 = 10_000_000;
const BOND: u64 = 100_000;
const WAIT: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(500);
const PAYMENT: u64 = 12_345;

pub(crate) fn rpc_addr(i: u16) -> String {
    format!("127.0.0.1:{}", RPC_BASE + i)
}

fn rpc(i: u16, method: &str, params: &Value) -> Result<Value> {
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let (_, reply) = devnet::post(&rpc_addr(i), "/", &request)?;
    match reply.get("error") {
        Some(error) => Err(format!("node {i} {method}: {error}")),
        None => Ok(reply.get("result").cloned().unwrap_or(Value::Null)),
    }
}

/// Keys, a funded wallet and a four-validator genesis. Returns the wallet
/// address.
pub(crate) fn setup(work: &Path) -> Result<String> {
    if work.exists() {
        std::fs::remove_dir_all(work).map_err(|e| format!("clearing {}: {e}", work.display()))?;
    }
    let mut validators = Vec::new();
    for i in 0..VALIDATORS {
        let dir = work.join(format!("v{i}"));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let key = devnet::output(
            Command::new(devnet::bin("maya2c-node"))
                .arg("--generate-validator-key")
                .arg(dir.join("validator.key")),
        )?;
        validators.push(key.trim().to_owned());
    }
    let generated = devnet::output(devnet::wallet(work).arg("generate"))?;
    let address =
        devnet::field(&generated, "address").ok_or("l1-wallet generate printed no address")?;
    let bonds: Vec<Value> = validators
        .iter()
        .map(|_| json!({ "operator": address, "bond": BOND }))
        .collect();
    let genesis = json!({
        "chain_id": "maya2c-localnet",
        "timestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        "difficulty_bits": 0,
        "pow_limit_bits": 0,
        "allocations": [{ "address": address, "balance": GENESIS_BALANCE }],
        "bft": {
            "validators": validators, "anchor_timeout_ms": 1000, "batch_size": 500,
            "staking": { "epoch_blocks": 20, "bonds": bonds }
        }
    });
    std::fs::write(work.join("genesis.json"), genesis.to_string())
        .map_err(|e| format!("writing genesis: {e}"))?;
    Ok(address)
}

pub(crate) fn start(work: &Path, i: u16) -> Result<Child> {
    let dir = work.join(format!("v{i}"));
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("node.log"))
        .map_err(|e| format!("node {i} log: {e}"))?;
    let mut cmd = Command::new(devnet::bin("maya2c-node"));
    cmd.arg("--genesis")
        .arg(work.join("genesis.json"))
        .arg("--data-dir")
        .arg(&dir)
        .args([
            "--rpc-addr",
            &rpc_addr(i),
            "--p2p-port",
            &(P2P_BASE + i).to_string(),
        ])
        .arg("--validator-key")
        .arg(dir.join("validator.key"));
    for peer in (0..VALIDATORS).filter(|p| *p != i) {
        cmd.args([
            "--bootnode",
            &format!("/ip4/127.0.0.1/tcp/{}", P2P_BASE + peer),
        ]);
    }
    cmd.stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("starting node {i}: {e}"))
}

/// `(height, block id)` of node `i`'s tip.
fn tip(i: u16, address: &str) -> Result<(u64, String)> {
    let v = rpc(i, "get_account_at_tip", &json!([address]))?;
    let height = v["height"].as_u64().ok_or("no height")?;
    let id = v["block_id"].as_str().ok_or("no block_id")?.to_owned();
    Ok((height, id))
}

fn block_id(i: u16, height: u64) -> Result<String> {
    let b = rpc(i, "get_block_by_height", &json!([height]))?;
    Ok(b["header"]["id"].as_str().ok_or("no header id")?.to_owned())
}

fn balance(i: u16, address: &str) -> Result<u64> {
    rpc(i, "get_balance", &json!([address]))?["balance"]
        .as_u64()
        .ok_or_else(|| format!("node {i}: get_balance carries no balance"))
}

/// Waits until `check` holds, polling.
pub(crate) fn wait_until(what: &str, mut check: impl FnMut() -> bool) -> Result<Duration> {
    let started = Instant::now();
    while started.elapsed() < WAIT {
        if check() {
            return Ok(started.elapsed());
        }
        std::thread::sleep(POLL);
    }
    Err(format!("{what}: not within {WAIT:?}"))
}

/// The nodes in `nodes` agree on the block at the lowest tip among them, and
/// that tip is at least `min_height`.
pub(crate) fn agree(nodes: &[u16], address: &str, min_height: u64) -> bool {
    let tips: Vec<(u64, String)> = nodes.iter().filter_map(|i| tip(*i, address).ok()).collect();
    if tips.len() != nodes.len() {
        return false;
    }
    let low = tips.iter().map(|t| t.0).min().unwrap_or(0);
    if low < min_height {
        return false;
    }
    let ids: Vec<String> = nodes
        .iter()
        .filter_map(|i| block_id(*i, low).ok())
        .collect();
    ids.len() == nodes.len() && ids.iter().all(|id| *id == ids[0])
}

fn run(work: &Path) -> Result<()> {
    devnet::build(&["maya2c-node", "l1-wallet"])?;
    let address = setup(work)?;
    let mut procs = Procs::default();
    let started = Instant::now();
    for i in 0..VALIDATORS {
        procs.push(start(work, i)?);
    }
    let all: Vec<u16> = (0..VALIDATORS).collect();
    let t = wait_until("four nodes on one chain", || agree(&all, &address, 3))?;
    println!("localnet: {VALIDATORS} validators agree on one chain {t:?} after start");

    let recipient = "5a".repeat(32);
    if !devnet::send(work, &recipient, PAYMENT) {
        return Err("the transfer was refused".into());
    }
    let t = wait_until("the transfer on every node", || {
        all.iter()
            .all(|i| balance(*i, &recipient).ok() == Some(PAYMENT))
    })?;
    println!("localnet: a transfer sent to node 0 executed on all {VALIDATORS} in {t:?}");

    procs.kill(usize::from(VALIDATORS - 1));
    let survivors: Vec<u16> = (0..VALIDATORS - 1).collect();
    let before = tip(0, &address)?.0;
    let t = wait_until("three validators without the fourth", || {
        agree(&survivors, &address, before + 5)
    })?;
    println!(
        "localnet: node {} killed; the other three committed 5 more blocks in {t:?}",
        VALIDATORS - 1
    );

    procs.replace(usize::from(VALIDATORS - 1), start(work, VALIDATORS - 1)?);
    let target = tip(0, &address)?.0;
    let t = wait_until("the restarted validator catching up", || {
        agree(&all, &address, target)
    })?;
    println!(
        "localnet: node {} restarted and caught up to height {target} in {t:?}",
        VALIDATORS - 1
    );
    println!("localnet: PASS in {:?}", started.elapsed());
    Ok(())
}

/// Runs the rehearsal in `target/localnet`.
///
/// # Errors
///
/// The first check that fails, with the node logs left in place.
pub fn localnet() -> Result<()> {
    let work = devnet::root().join("target").join("localnet");
    run(&work).map_err(|e| format!("{e} (logs: {})", work.display()))
}
