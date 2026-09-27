//! `cargo xtask mesh-check` — `mesh-cli check:data` against a local devnet
//! (Master Prompt 17).
//!
//! Starts a one-validator DAG-BFT devnet and `maya2c-mesh`, then runs
//! mesh-cli with reconciliation on, and historical balance lookup on, while
//! transfers keep landing until mesh-cli exits. (With historical lookup off,
//! mesh-cli reconciles only at the tip, and a devnet producing blocks as fast
//! as mesh-cli syncs them never let it get there: 56 of 824 blocks in 900 s.)
//!
//! ```text
//! cargo xtask mesh-check [--mesh-cli PATH] [--timeout SECS] [--workdir DIR]
//! ```

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::devnet::{self, Procs};

const MESH_ADDR: &str = "127.0.0.1:32081";
const NETWORK: &str = "maya-bft-devnet";
/// Distinct recipients: the end condition needs all of them reconciled.
const RECIPIENTS: u8 = 20;
const SEND_EVERY: Duration = Duration::from_millis(1_500);

struct Options {
    mesh_cli: PathBuf,
    timeout: Duration,
    work: PathBuf,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        mesh_cli: PathBuf::from("D:/Tools/mesh-cli/mesh-cli.exe"),
        timeout: Duration::from_secs(900),
        work: PathBuf::from("D:/Temp/maya-mesh"),
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let value = it.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--mesh-cli" => o.mesh_cli = PathBuf::from(value),
            "--workdir" => o.work = PathBuf::from(value),
            "--timeout" => {
                o.timeout =
                    Duration::from_secs(value.parse().map_err(|e| format!("--timeout: {e}"))?);
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(o)
}

fn recipient(i: u8) -> String {
    format!("{:02x}", 0x31 + i).repeat(32)
}

/// Genesis allocations are the balances before block 1: bonds are stake
/// records, not debits (checked against a run's first recorded changes).
fn write_bootstrap(work: &Path) -> Result<(), String> {
    let genesis: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(work.join("genesis.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let balances: Vec<_> = genesis["allocations"]
        .as_array()
        .ok_or("genesis has no allocations")?
        .iter()
        .map(|a| {
            json!({
                "account_identifier": { "address": a["address"] },
                "currency": { "symbol": "MAYA", "decimals": 0 },
                "value": a["balance"].to_string(),
            })
        })
        .collect();
    std::fs::write(work.join("bootstrap.json"), json!(balances).to_string())
        .map_err(|e| e.to_string())
}

fn write_config(work: &Path) -> Result<PathBuf, String> {
    let config = json!({
        "network": { "blockchain": "maya2c", "network": NETWORK },
        "online_url": format!("http://{MESH_ADDR}"),
        // Relative: mesh-cli joins these onto the config file's directory,
        // and an absolute `D:/...` came out as `D:/Temp/D:/Temp/...`.
        "data_directory": "mesh-data",
        "http_timeout": 10, "max_retries": 5, "max_online_connections": 16,
        "max_sync_concurrency": 4, "tip_delay": 300,
        "data": {
            "historical_balance_disabled": false,
            "bootstrap_balances": "bootstrap.json",
            "active_reconciliation_concurrency": 4,
            "inactive_reconciliation_concurrency": 4,
            // Every block, not every 250 (the default): an account paid once
            // early is otherwise not re-checked within a devnet run.
            "inactive_reconciliation_frequency": 1,
            // Every account seen, and not before every recipient has been
            // paid. Not `index`: in mesh-cli v0.10.4 `if *Index < blockIndex
            // { continue }` makes it a maximum, not the documented minimum.
            "end_conditions": { "reconciliation_coverage": {
                "coverage": 1.0, "account_count": u64::from(RECIPIENTS) + 1 } }
        }
    });
    let path = work.join("mesh-config.json");
    std::fs::write(&path, config.to_string()).map_err(|e| e.to_string())?;
    Ok(path)
}

fn check(o: &Options) -> Result<bool, String> {
    devnet::build(&["maya2c-node", "l1-wallet", "maya-mesh-api"])?;
    devnet::setup(&o.work, NETWORK)?;
    let mut procs = Procs::default();
    procs.push(devnet::start_node(&o.work)?);
    let rpc = format!("http://127.0.0.1:{}", devnet::RPC_PORT);
    procs.push(devnet::start(
        &o.work,
        "maya2c-mesh",
        &["--node", &rpc, "--network", NETWORK, "--listen", MESH_ADDR],
    )?);
    devnet::wait_for_http(MESH_ADDR)?;
    devnet::wait_for_first_block()?;
    write_bootstrap(&o.work)?;

    let log = std::fs::File::create(o.work.join("mesh-cli.log")).map_err(|e| e.to_string())?;
    let mut cli = Command::new(&o.mesh_cli)
        .arg("check:data")
        .arg("--configuration-file")
        // By bare name from the devnet directory: mesh-cli takes the config's
        // directory with a `/`-only split, so `D:/Temp/maya-mesh\mesh-config.json`
        // resolved its relative paths against `D:/Temp`.
        .arg(
            write_config(&o.work)?
                .file_name()
                .ok_or("config path has no file name")?,
        )
        .current_dir(&o.work)
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| format!("starting {}: {e}", o.mesh_cli.display()))?;

    let deadline = Instant::now() + o.timeout;
    let (mut sent, mut n) = (0u32, 0u64);
    let status = loop {
        if let Some(status) = cli.try_wait().map_err(|e| e.to_string())? {
            break Some(status);
        }
        if Instant::now() > deadline {
            let _ = cli.kill();
            break None;
        }
        let to = recipient(u8::try_from(n % u64::from(RECIPIENTS)).unwrap_or(0));
        if devnet::send(&o.work, &to, 1_000 + n) {
            sent += 1;
        }
        n += 1;
        std::thread::sleep(SEND_EVERY);
    };
    println!("sent {sent} transfers");
    let text = std::fs::read_to_string(o.work.join("mesh-cli.log")).unwrap_or_default();
    let tail: String = text
        .lines()
        .rev()
        .take(60)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    println!("{tail}");
    match status {
        None => Err(format!("mesh-cli did not finish within {:?}", o.timeout)),
        Some(status) => Ok(status.success() && !text.contains("FAILED")),
    }
}

/// Entry point.
///
/// # Errors
///
/// A setup failure, a timeout, or mesh-cli reporting a failure.
pub fn run(args: &[String]) -> Result<(), String> {
    if check(&options(args)?)? {
        println!("mesh-check: passed");
        Ok(())
    } else {
        Err("mesh-cli reported a failure (log above)".into())
    }
}
