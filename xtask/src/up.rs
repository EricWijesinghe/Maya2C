//! `cargo xtask up` / `cargo xtask down` — the whole ecosystem on this
//! machine, in one command, left running.
//!
//! `up` builds the binaries, then starts:
//!
//! - four `maya2c-node` validators with DAG-BFT finality over real libp2p on
//!   127.0.0.1 (the same genesis `cargo xtask localnet` rehearses), with a
//!   funded devnet wallet;
//! - `maya2c-gateway`, the public REST/GraphQL surface, in front of node 0;
//! - a `maya-chat` relay (ADR-031);
//! - the testnet faucet, funded in genesis.
//!
//! Fees are on, with the public testnet installer's parameters, so what works
//! here is what will work on the testnet.
//!
//! It waits until the validators agree on a chain and every service answers,
//! then prints the endpoints and writes `target/up/up.json`. `down` stops
//! exactly what `up` started, checking each process's name first so a
//! process id the OS has since reused is never killed.
//!
//! This is a local devnet: keys are generated fresh each time and nothing is
//! reachable from outside this machine. It is not a deployment.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::devnet::{self, Procs, Result};
use crate::localnet;

const GATEWAY_ADDR: &str = "127.0.0.1:8080";
const CHAT_RELAY_LISTEN: &str = "/ip4/127.0.0.1/tcp/4001";
const FAUCET_ADDR: &str = "127.0.0.1:8090";
const FAUCET_BALANCE: u64 = 5_000_000;
const CHAIN_ID: &str = "maya2c-localnet";
/// First chain height the validators must agree on before `up` reports.
const FIRST_AGREED_HEIGHT: u64 = 3;

fn work() -> PathBuf {
    devnet::root().join("target").join("up")
}

fn state_file(work: &Path) -> PathBuf {
    work.join("up.json")
}

/// One process `up` started: its label, id and executable name.
fn service(label: &str, pid: u32, image: &str) -> Value {
    json!({ "label": label, "pid": pid, "image": image })
}

/// Waits for `log` to contain a line starting with `prefix`; returns the
/// rest of that line.
fn wait_for_line(log: &Path, prefix: &str) -> Result<String> {
    let deadline = Instant::now() + devnet::BOOT_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(log)
            && let Some(rest) = text.lines().find_map(|l| l.strip_prefix(prefix))
        {
            return Ok(rest.trim().to_owned());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(format!("{} printed no `{prefix}` line", log.display()))
}

/// Turns fees on in the genesis `localnet::setup` wrote, with the testnet
/// installer's parameters, and funds a fresh faucet key. Returns the faucet
/// address; the key stays in `faucet.env`.
fn fees_and_faucet(work: &Path) -> Result<String> {
    let faucet = devnet::output(
        Command::new(devnet::bin("maya-faucet"))
            .arg("generate-key")
            .arg(work.join("faucet.env")),
    )?
    .trim()
    .to_owned();
    let path = work.join("genesis.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("genesis: {e}"))?;
    let mut genesis: Value = serde_json::from_str(&text).map_err(|e| format!("genesis: {e}"))?;
    genesis["bft"]["fees"] = json!({
        "initial_base_fee": 1, "min_base_fee": 1,
        "target_block_bytes": 2_621_440, "change_denominator": 8
    });
    genesis["allocations"]
        .as_array_mut()
        .ok_or("genesis has no allocations")?
        .push(json!({ "address": faucet, "balance": FAUCET_BALANCE }));
    std::fs::write(&path, genesis.to_string()).map_err(|e| format!("genesis: {e}"))?;
    Ok(faucet)
}

/// Starts the faucet with its key from `faucet.env`, passed through the
/// environment (never a flag, which other processes can read).
fn start_faucet(work: &Path, procs: &mut Procs) -> Result<()> {
    let env =
        std::fs::read_to_string(work.join("faucet.env")).map_err(|e| format!("faucet key: {e}"))?;
    let key = env
        .lines()
        .find_map(|l| l.strip_prefix("MAYA_FAUCET_KEY="))
        .ok_or("faucet.env holds no key")?;
    let log = std::fs::File::create(work.join("maya-faucet.log"))
        .map_err(|e| format!("faucet log: {e}"))?;
    let child = Command::new(devnet::bin("maya-faucet"))
        .env("MAYA_FAUCET_KEY", key.trim())
        .env("MAYA_FAUCET_CHAIN", CHAIN_ID)
        .env(
            "MAYA_FAUCET_NODE",
            format!("http://{}", localnet::rpc_addr(0)),
        )
        .env("MAYA_FAUCET_LISTEN", FAUCET_ADDR)
        .stdout(log.try_clone().map_err(|e| format!("faucet log: {e}"))?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("starting maya-faucet: {e}"))?;
    procs.push(child);
    devnet::wait_for_http(FAUCET_ADDR)
}

fn start_validators(work: &Path, procs: &mut Procs) -> Result<(String, Duration)> {
    let address = localnet::setup(work)?;
    fees_and_faucet(work)?;
    for i in 0..localnet::VALIDATORS {
        procs.push(localnet::start(work, i)?);
    }
    let all: Vec<u16> = (0..localnet::VALIDATORS).collect();
    let took = localnet::wait_until("validators agreeing on one chain", || {
        localnet::agree(&all, &address, FIRST_AGREED_HEIGHT)
    })?;
    Ok((address, took))
}

fn start_chat_relay(work: &Path, procs: &mut Procs) -> Result<String> {
    let home = work.join("chat-relay");
    let home = home.to_str().ok_or("work path is not UTF-8")?;
    procs.push(devnet::start(
        work,
        "maya-chat",
        &["--home", home, "relay", "--listen", CHAT_RELAY_LISTEN],
    )?);
    wait_for_line(&work.join("maya-chat.log"), "relay listening on ")
}

fn bring_up(work: &Path) -> Result<Value> {
    devnet::build(&[
        "maya2c-node",
        "l1-wallet",
        "maya-api-gateway",
        "maya-chat",
        "maya-faucet",
    ])?;
    let mut procs = Procs::default();
    let (wallet, took) = start_validators(work, &mut procs)?;
    let node0 = format!("http://{}", localnet::rpc_addr(0));
    procs.push(devnet::start(
        work,
        "maya2c-gateway",
        &["--node", &node0, "--listen", GATEWAY_ADDR],
    )?);
    devnet::wait_for_http(GATEWAY_ADDR)?;
    let relay = start_chat_relay(work, &mut procs)?;
    start_faucet(work, &mut procs)?;

    let pids = procs.detach();
    let images = ["maya2c-node"; localnet::VALIDATORS as usize]
        .into_iter()
        .chain(["maya2c-gateway", "maya-chat", "maya-faucet"]);
    let labels = (0..localnet::VALIDATORS)
        .map(|i| format!("validator-{i}"))
        .chain([
            "gateway".to_owned(),
            "chat-relay".to_owned(),
            "faucet".to_owned(),
        ]);
    let services: Vec<Value> = labels
        .zip(images)
        .zip(pids)
        .map(|((label, image), pid)| service(&label, pid, image))
        .collect();
    Ok(json!({
        "services": services,
        "agreed_after_secs": took.as_secs_f64(),
        "endpoints": {
            "node_rpc": (0..localnet::VALIDATORS)
                .map(|i| format!("http://{}", localnet::rpc_addr(i)))
                .collect::<Vec<_>>(),
            "gateway": format!("http://{GATEWAY_ADDR}"),
            "chat_relay": relay,
            "faucet": format!("http://{FAUCET_ADDR}"),
        },
        "wallet": { "address": wallet, "keystore": work.join("wallet").display().to_string() },
        "logs": work.display().to_string(),
    }))
}

fn print_summary(state: &Value) {
    let e = &state["endpoints"];
    println!("up: the ecosystem is running (local devnet; fresh keys; loopback only)");
    println!(
        "  validators  {} (agreed after {:.1} s)",
        localnet::VALIDATORS,
        state["agreed_after_secs"].as_f64().unwrap_or_default()
    );
    for rpc in e["node_rpc"].as_array().into_iter().flatten() {
        println!("  node RPC    {}", rpc.as_str().unwrap_or_default());
    }
    println!(
        "  gateway     {}",
        e["gateway"].as_str().unwrap_or_default()
    );
    println!(
        "  chat relay  {}",
        e["chat_relay"].as_str().unwrap_or_default()
    );
    println!(
        "  faucet      {} (POST /request {{\"address\": \"<hex>\"}})",
        e["faucet"].as_str().unwrap_or_default()
    );
    println!(
        "  wallet      {} (funded; devnet keystore in {})",
        state["wallet"]["address"].as_str().unwrap_or_default(),
        state["wallet"]["keystore"].as_str().unwrap_or_default()
    );
    println!(
        "  logs        {}",
        state["logs"].as_str().unwrap_or_default()
    );
    println!("stop it with: cargo xtask down");
}

/// Starts everything and leaves it running.
///
/// # Errors
///
/// Already up, a build failure, or a service that did not come up (in which
/// case everything started so far is stopped again).
pub fn up() -> Result<()> {
    let work = work();
    if state_file(&work).exists() {
        return Err("already up (target/up/up.json exists); run `cargo xtask down` first".into());
    }
    let state = bring_up(&work).map_err(|e| format!("{e} (logs: {})", work.display()))?;
    std::fs::write(
        state_file(&work),
        serde_json::to_string_pretty(&state).unwrap_or_default(),
    )
    .map_err(|e| format!("writing up.json: {e}"))?;
    print_summary(&state);
    Ok(())
}

/// The executable name running as `pid`, if any.
fn image_of(pid: u32) -> Option<String> {
    let out = if cfg!(windows) {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
    } else {
        Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
    }
    .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next()?.trim();
    let name = first.trim_start_matches('"').split('"').next()?;
    (!name.is_empty() && !name.starts_with("INFO:"))
        .then(|| name.trim_end_matches(".exe").to_owned())
}

fn kill(pid: u32) -> bool {
    let status = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
    } else {
        Command::new("kill").arg(pid.to_string()).output()
    };
    status.is_ok_and(|o| o.status.success())
}

/// Stops what `up` started.
///
/// # Errors
///
/// No `up.json`, or a process that should have stopped did not.
pub fn down() -> Result<()> {
    let path = state_file(&work());
    let text = std::fs::read_to_string(&path)
        .map_err(|_| "nothing to stop: target/up/up.json does not exist".to_owned())?;
    let state: Value = serde_json::from_str(&text).map_err(|e| format!("up.json: {e}"))?;
    let mut failed = Vec::new();
    for s in state["services"].as_array().into_iter().flatten() {
        let (label, image) = (
            s["label"].as_str().unwrap_or("?"),
            s["image"].as_str().unwrap_or(""),
        );
        let Some(pid) = s["pid"].as_u64().and_then(|p| u32::try_from(p).ok()) else {
            continue;
        };
        match image_of(pid) {
            Some(running) if running == image => {
                if kill(pid) {
                    println!("down: stopped {label} (pid {pid})");
                } else {
                    failed.push(format!("{label} (pid {pid})"));
                }
            }
            Some(other) => println!("down: {label} gone; pid {pid} is now `{other}`, left alone"),
            None => println!("down: {label} (pid {pid}) had already exited"),
        }
    }
    if !failed.is_empty() {
        return Err(format!("could not stop: {}", failed.join(", ")));
    }
    std::fs::remove_file(&path).map_err(|e| format!("removing up.json: {e}"))?;
    println!("down: everything `up` started is stopped");
    Ok(())
}
