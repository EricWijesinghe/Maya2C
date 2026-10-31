//! `cargo xtask proving [--hours N] [--gpu]` — the multi-day proving run on one
//! machine, with the genesis built exactly as mainnet's will be.
//!
//! Seven `maya2c-node` validators, each with its own operator address, from a
//! genesis written by `genesis-ceremony` with `deploy/genesis/mainnet/mining.json`
//! and the mainnet staking floors and fee floor, checked by `genesis-check`.
//! Throwaway keys, a non-value chain id (`maya-proving-1`), and stake-weighted
//! voting switched on afterwards as mainnet's id does (only a value-bearing id
//! gets it from the tool). A `maya2c-gpu-miner` mines into node 1.
//!
//! Every minute it reads every node's tip. It fails at once on a fork (two
//! nodes holding different blocks at one height). It records each halt (no
//! new block for [`HALT_AFTER`]) with its length. Each hour it appends a line
//! to `reports/proving/<date>.md`: height, epoch, miner balance, reserve.
//!
//! What this is not: separate machines, real latency, or other operators. It
//! runs the mainnet genesis and binaries for days on one PC, which no
//! minutes-long test does.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::devnet::{self, Procs, Result};

const VALIDATORS: u16 = 7;
/// Clear of the attacknet (32000/32300), localnet (31100) and mining-localnet
/// (33100/34000).
const RPC_BASE: u16 = 35_000;
const P2P_BASE: u16 = 35_300;
const CHAIN_ID: &str = "maya-proving-1";
const COIN: u128 = 1_000_000_000_000_000_000_000_000;
/// 10,000 coins per validator: above the 1,000-coin floor, 1/7 of the stake each.
const BOND: u128 = 10_000 * COIN;
/// The root allocation: fees for the run's transfers, nothing more.
const SUPPLY: u128 = 1_000 * COIN;
/// `MiningRecord::lifetime_emission` of the mainnet schedule (README beside it).
const RESERVE: &str = "122753829695359067419575633036805965021648";
const MIN_SELF_BOND: u128 = 1_000 * COIN;
const MIN_DELEGATION: u128 = COIN;
const DEFAULT_HOURS: u64 = 120;
const CHECK_EVERY: Duration = Duration::from_secs(60);
const REPORT_EVERY: Duration = Duration::from_secs(3_600);
/// No new block for this long is a halt.
const HALT_AFTER: Duration = Duration::from_secs(300);
const BOOT_WAIT: Duration = Duration::from_secs(300);
const PASSWORD: &str = "proving-only-password";
/// `MINING_RESERVE`: "maya2c/mining/reserve/v1" zero-padded to 32 bytes.
const RESERVE_ADDRESS: &str = "6d61796132632f6d696e696e672f726573657276652f76310000000000000000";

fn rpc_addr(i: u16) -> String {
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

fn height(i: u16) -> Option<u64> {
    rpc(i, "get_tip_height", &json!([])).ok()?.as_u64()
}

fn epoch(i: u16) -> Option<u64> {
    rpc(i, "get_bft_status", &json!([])).ok()?["epoch"].as_u64()
}

fn block_id(i: u16, h: u64) -> Option<String> {
    let b = rpc(i, "get_block_by_height", &json!([h])).ok()?;
    b["header"]["id"].as_str().map(str::to_owned)
}

/// A balance as the node reports it: a decimal string of base units, kept as a
/// string because mainnet amounts pass `u64` at the first coin.
fn balance(i: u16, address: &str) -> String {
    rpc(i, "get_balance", &json!([address]))
        .ok()
        .and_then(|v| v["balance"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "?".to_owned())
}

/// A new `l1-wallet` keystore; returns its address.
fn new_wallet(work: &Path, name: &str) -> Result<String> {
    let out = devnet::output(
        Command::new(devnet::bin("l1-wallet"))
            .env("L1_WALLET_PASSWORD", PASSWORD)
            .arg("--keystore")
            .arg(work.join(format!("{name}.key")))
            .arg("generate"),
    )?;
    devnet::field(&out, "address").ok_or_else(|| format!("{name}: no address printed"))
}

/// Keys, the ceremony and `genesis-check`. Returns the genesis path.
fn setup(work: &Path, root: &Path) -> Result<PathBuf> {
    if work.exists() {
        std::fs::remove_dir_all(work).map_err(|e| format!("clearing {}: {e}", work.display()))?;
    }
    let mut lines = String::new();
    for i in 0..VALIDATORS {
        let dir = work.join(format!("v{i}"));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let key = devnet::output(
            Command::new(devnet::bin("maya2c-node"))
                .arg("--generate-validator-key")
                .arg(dir.join("validator.key")),
        )?;
        let operator = new_wallet(work, &format!("operator{i}"))?;
        let _ = writeln!(lines, "v{i} {} {operator} {BOND}", key.trim());
    }
    std::fs::write(work.join("validators.txt"), lines).map_err(|e| e.to_string())?;

    // Three council keys, public halves only in the folder the tool reads.
    let council = work.join("council");
    let keys = work.join("council-keys");
    std::fs::create_dir_all(&council).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&keys).map_err(|e| e.to_string())?;
    let pass = work.join("council-pass");
    std::fs::write(&pass, PASSWORD).map_err(|e| e.to_string())?;
    for label in ["c1", "c2", "c3"] {
        devnet::output(
            Command::new(devnet::bin("genesis-ceremony"))
                .args(["council", "key", "--label", label, "--passphrase-file"])
                .arg(&pass)
                .current_dir(&keys),
        )?;
        let public = format!("{label}.council.public");
        std::fs::copy(keys.join(&public), council.join(&public)).map_err(|e| e.to_string())?;
    }

    let out = work.join("ceremony");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    devnet::output(
        Command::new(devnet::bin("genesis-ceremony"))
            .args(["--chain-id", CHAIN_ID, "--supply", &SUPPLY.to_string()])
            .arg("--validators")
            .arg(work.join("validators.txt"))
            .arg("--mining")
            .arg(root.join("deploy/genesis/mainnet/mining.json"))
            .args(["--mining-reserve", RESERVE, "--council"])
            .arg(&council)
            .args([
                "--council-threshold",
                "2",
                "--council-max-pause-blocks",
                "7200",
            ])
            .args(["--min-self-bond", &MIN_SELF_BOND.to_string()])
            .args(["--min-delegation", &MIN_DELEGATION.to_string()])
            .args(["--timestamp", &now.to_string(), "--out-dir"])
            .arg(&out),
    )?;
    // Mainnet's id gets stake weighting from the tool; this id does not, so
    // it is set here, the one difference from a mainnet genesis.
    let path = out.join("genesis.json");
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut genesis: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    genesis["bft"]["staking"]["stake_weighted"] = json!(true);
    std::fs::write(&path, genesis.to_string()).map_err(|e| e.to_string())?;
    crate::genesis_check::run(&[path.to_string_lossy().into_owned()])?;
    Ok(path)
}

fn start_node(work: &Path, genesis: &Path, i: u16) -> Result<std::process::Child> {
    let dir = work.join(format!("v{i}"));
    let log = std::fs::File::create(dir.join("node.log")).map_err(|e| e.to_string())?;
    let mut cmd = Command::new(devnet::bin("maya2c-node"));
    cmd.arg("--genesis")
        .arg(genesis)
        .arg("--data-dir")
        .arg(&dir)
        .args(["--rpc-addr", &rpc_addr(i)])
        .args(["--p2p-port", &(P2P_BASE + i).to_string()])
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

/// The lowest tip every node holds, after checking they all hold the same
/// block there. `Err` on a fork; `Ok(None)` while a node is unreachable.
fn agreed_height() -> Result<Option<u64>> {
    let tips: Vec<u64> = (0..VALIDATORS).filter_map(height).collect();
    if tips.len() != usize::from(VALIDATORS) {
        return Ok(None);
    }
    let low = tips.iter().copied().min().unwrap_or(0);
    let ids: Vec<Option<String>> = (0..VALIDATORS).map(|i| block_id(i, low)).collect();
    if ids.iter().any(Option::is_none) {
        return Ok(None);
    }
    if ids.windows(2).any(|w| w[0] != w[1]) {
        return Err(format!("FORK at height {low}: {ids:?}"));
    }
    Ok(Some(low))
}

struct Report {
    path: PathBuf,
    halts: Vec<Duration>,
}

impl Report {
    fn line(&self, text: &str) {
        println!("proving: {text}");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path);
        if let Ok(f) = file.as_mut() {
            use std::io::Write as _;
            let _ = writeln!(f, "- {text}");
        }
    }
}

pub fn proving(args: &[String]) -> Result<()> {
    let hours = args
        .iter()
        .position(|a| a == "--hours")
        .and_then(|i| args.get(i + 1))
        .map_or(Ok(DEFAULT_HOURS), |s| s.parse::<u64>())
        .map_err(|e| format!("--hours: {e}"))?;
    let gpu = args.iter().any(|a| a == "--gpu");
    let root = devnet::root();
    let work = root.join("target").join("proving");
    run(&root, &work, hours, gpu).map_err(|e| format!("{e} (logs: {})", work.display()))
}

fn run(root: &Path, work: &Path, hours: u64, gpu: bool) -> Result<()> {
    devnet::build(&[
        "maya2c-node",
        "l1-wallet",
        "genesis-ceremony",
        "maya2c-gpu-miner",
    ])?;
    let genesis = setup(work, root)?;
    let date = chrono_date();
    let dir = root.join("reports").join("proving");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut report = Report {
        path: dir.join(format!("{date}.md")),
        halts: Vec::new(),
    };
    let commit = devnet::output(Command::new("git").args(["rev-parse", "--short", "HEAD"]))
        .unwrap_or_default();
    std::fs::write(
        &report.path,
        format!(
            "# Proving run {date}\n\n- commit: {}\n- chain: {CHAIN_ID}, {VALIDATORS} validators, one machine, throwaway keys\n- genesis: genesis-ceremony + deploy/genesis/mainnet/mining.json, genesis-check PASS\n- planned: {hours} h\n\n",
            commit.trim()
        ),
    )
    .map_err(|e| e.to_string())?;

    let mut nodes = Procs::default();
    for i in 0..VALIDATORS {
        nodes.push(start_node(work, &genesis, i)?);
    }
    let started = Instant::now();
    while agreed_height()?.is_none_or(|h| h < 3) {
        if started.elapsed() > BOOT_WAIT {
            return Err("the seven validators never agreed on height 3".into());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    report.line(&format!(
        "{VALIDATORS} validators on one chain after {:.0?}",
        started.elapsed()
    ));

    let mut miner = Procs::default();
    let mut cmd = Command::new(devnet::bin("maya2c-gpu-miner"));
    cmd.args(["--mine", "--json", "--threads", "4", "--rpc"])
        .arg(format!("http://{}", rpc_addr(1)))
        .arg("--key-file")
        .arg(work.join("miner.key"))
        .stdout(std::fs::File::create(work.join("miner.jsonl")).map_err(|e| e.to_string())?)
        .stderr(Stdio::null());
    if gpu {
        cmd.arg("--gpu");
    }
    miner.push(cmd.spawn().map_err(|e| format!("starting miner: {e}"))?);

    let deadline = Duration::from_secs(hours * 3_600);
    let mut last_height = 0;
    let mut last_progress = Instant::now();
    let mut halted_since: Option<Instant> = None;
    let mut next_report = Instant::now();
    while started.elapsed() < deadline {
        std::thread::sleep(CHECK_EVERY);
        if let Some(h) = agreed_height()?
            && h > last_height
        {
            if let Some(since) = halted_since.take() {
                report.halts.push(since.elapsed());
                report.line(&format!(
                    "halt ended after {:.0?}, at height {h}",
                    since.elapsed()
                ));
            }
            last_height = h;
            last_progress = Instant::now();
        }
        if halted_since.is_none() && last_progress.elapsed() > HALT_AFTER {
            halted_since = Some(last_progress);
            report.line(&format!(
                "HALT: no block past {last_height} for {HALT_AFTER:?}"
            ));
        }
        if Instant::now() >= next_report {
            next_report = Instant::now() + REPORT_EVERY;
            report.line(&format!(
                "{:.1} h: height {last_height}, epoch {}, reserve {}, halts {}",
                started.elapsed().as_secs_f64() / 3_600.0,
                epoch(0).map_or_else(|| "?".into(), |e| e.to_string()),
                balance(0, RESERVE_ADDRESS),
                report.halts.len()
            ));
        }
    }
    drop(miner);
    let total: Duration = report.halts.iter().sum();
    report.line(&format!(
        "PASS: {hours} h, no fork, height {last_height}, {} halt(s) totalling {total:.0?}",
        report.halts.len()
    ));
    Ok(())
}

/// Today's date, UTC, as `YYYY-MM-DD`, from the system clock (a report name,
/// not a consensus input).
fn chrono_date() -> String {
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() / 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserve_address_is_the_padded_tag() {
        let tag: String = b"maya2c/mining/reserve/v1"
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(RESERVE_ADDRESS.len(), 64);
        assert!(RESERVE_ADDRESS.starts_with(&tag));
        assert!(RESERVE_ADDRESS[tag.len()..].chars().all(|c| c == '0'));
    }

    #[test]
    fn the_report_date_is_a_civil_date() {
        let d = chrono_date();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20"));
    }
}
