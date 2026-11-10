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
//! exactly what `up` started, by process id, and only where the process's
//! start time still matches the one recorded. Nothing is matched by name:
//! a process id the OS has since reused is never killed.
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
const EXPLORER_ADDR: &str = "127.0.0.1:3000";
const FAUCET_BALANCE: u64 = 5_000_000;
const CHAIN_ID: &str = "maya2c-localnet";
/// First chain height the validators must agree on before `up` reports.
const FIRST_AGREED_HEIGHT: u64 = 3;
/// The fixed-port services `up` binds, checked before anything starts. The
/// chat relay's multiaddr listens on the same 4001 as `CHAT_RELAY_LISTEN`.
const SERVICE_ADDRS: [&str; 4] = [GATEWAY_ADDR, FAUCET_ADDR, EXPLORER_ADDR, "127.0.0.1:4001"];
/// Shown with the wallet path. Devnet-only; it protects nothing real.
const WALLET_PASSWORD_SOURCE: &str = "PASSWORD in xtask/src/devnet.rs";

pub(crate) fn work() -> PathBuf {
    devnet::root().join("target").join("up")
}

fn state_file(work: &Path) -> PathBuf {
    work.join("up.json")
}

/// Whether `up` is running (its state file exists).
pub(crate) fn is_up() -> bool {
    state_file(&work()).exists()
}

/// The state `up` wrote: services, endpoints, wallet.
pub(crate) fn read_state() -> Result<Value> {
    let text = std::fs::read_to_string(state_file(&work())).map_err(|_| {
        "not up: target/up/up.json does not exist (run `cargo xtask kit up`)".to_owned()
    })?;
    serde_json::from_str(&text).map_err(|e| format!("up.json: {e}"))
}

/// The addresses among `addrs` something already listens on.
fn ports_in_use(addrs: &[&str]) -> Vec<String> {
    addrs
        .iter()
        .filter(|a| std::net::TcpListener::bind(*a).is_err())
        .map(|a| (*a).to_owned())
        .collect()
}

/// Refuses to start when a validator or service port is taken: on the PC that
/// runs the live testnet, a second devnet would dial the live seed.
fn check_ports_free() -> Result<()> {
    localnet::ports_free()?;
    let busy = ports_in_use(&SERVICE_ADDRS);
    if busy.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "ports in use: {} (another node or devnet is running here)",
            busy.join(", ")
        ))
    }
}

/// One process `up` started: its label, PID and start time.
fn service(label: &str, pid: u32, started: &str) -> Value {
    json!({ "label": label, "pid": pid, "started": started })
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

/// The block explorer, reading node 0 into an in-memory index (no database).
fn start_explorer(work: &Path, node0: &str, procs: &mut Procs) -> Result<()> {
    let assets = devnet::root().join("apps").join("explorer").join("assets");
    let assets = assets.to_str().ok_or("explorer assets path is not UTF-8")?;
    procs.push(devnet::start(
        work,
        "explorer",
        &[
            "--node-rpc",
            node0,
            "--listen",
            EXPLORER_ADDR,
            "--assets",
            assets,
        ],
    )?);
    devnet::wait_for_http(EXPLORER_ADDR)
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
        "maya-explorer",
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
    start_explorer(work, &node0, &mut procs)?;

    let pids = procs.detach();
    // Recorded now, while each process is known to be the one just started:
    // `down` compares start times, because a PID alone is reused by the OS.
    let starts: Result<Vec<String>> = pids
        .iter()
        .map(|pid| {
            process_start(*pid)
                .ok_or_else(|| format!("process {pid} exited before it was recorded"))
        })
        .collect();
    let starts = match starts {
        Ok(starts) => starts,
        Err(e) => {
            // These are ours and were just started: stop them rather than orphan them.
            for pid in &pids {
                let _ = kill(*pid);
            }
            return Err(e);
        }
    };
    let labels = (0..localnet::VALIDATORS)
        .map(|i| format!("validator-{i}"))
        .chain([
            "gateway".to_owned(),
            "chat-relay".to_owned(),
            "faucet".to_owned(),
            "explorer".to_owned(),
        ]);
    let services: Vec<Value> = labels
        .zip(pids)
        .zip(starts)
        .map(|((label, pid), started)| service(&label, pid, &started))
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
            "explorer": format!("http://{EXPLORER_ADDR}"),
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
        "  explorer    {}",
        e["explorer"].as_str().unwrap_or_default()
    );
    println!(
        "  wallet      {} (funded; devnet keystore in {}; password: {WALLET_PASSWORD_SOURCE})",
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
    check_ports_free()?;
    let state = bring_up(&work).map_err(|e| format!("{e} (logs: {})", work.display()))?;
    std::fs::write(
        state_file(&work),
        serde_json::to_string_pretty(&state).unwrap_or_default(),
    )
    .map_err(|e| format!("writing up.json: {e}"))?;
    print_summary(&state);
    Ok(())
}

/// When the process `pid` started, as a string that stays fixed for its
/// lifetime. `None` when no such process runs. The OS reuses PIDs, so `down`
/// compares this value, never the number alone or the executable name.
pub(crate) fn process_start(pid: u32) -> Option<String> {
    if cfg!(windows) {
        let script = format!(
            "(Get-Process -Id {pid} -ErrorAction SilentlyContinue).StartTime.ToFileTimeUtc()"
        );
        let out = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (!text.is_empty()).then_some(text)
    } else {
        // /proc/<pid>/stat: after the parenthesised name, field 22 (starttime,
        // clock ticks since boot) is the 20th field.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_name = stat.rsplit_once(')')?.1;
        after_name.split_whitespace().nth(19).map(str::to_owned)
    }
}

/// What `down` does with one recorded process.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// The process is the one `up` started: stop it.
    Stop,
    /// No process holds that PID any more.
    Gone,
    /// A process holds the PID, but it is not ours (different start time, or
    /// none was recorded). Leave it alone.
    Reused,
}

pub(crate) fn action(recorded: Option<&str>, now: Option<&str>) -> Action {
    match (recorded, now) {
        (_, None) => Action::Gone,
        (Some(r), Some(n)) if r == n => Action::Stop,
        _ => Action::Reused,
    }
}

/// Stops each recorded process whose start time still matches. Returns how
/// many were left alone. `observe` and `kill` are the OS calls, passed in so
/// the rule can be tested without starting processes.
pub(crate) fn stop_recorded(
    state: &Value,
    observe: &dyn Fn(u32) -> Option<String>,
    kill: &dyn Fn(u32) -> bool,
) -> Result<usize> {
    let mut left_alone = 0;
    let mut failed = Vec::new();
    for s in state["services"].as_array().into_iter().flatten() {
        let label = s["label"].as_str().unwrap_or("?");
        let Some(pid) = s["pid"].as_u64().and_then(|p| u32::try_from(p).ok()) else {
            continue;
        };
        match action(s["started"].as_str(), observe(pid).as_deref()) {
            Action::Gone => println!("down: {label} (pid {pid}) had already exited"),
            Action::Reused => {
                left_alone += 1;
                println!(
                    "down: {label}: pid {pid} is not the process `up` started (start time differs or was not recorded); left alone"
                );
            }
            Action::Stop => {
                if kill(pid) {
                    println!("down: stopped {label} (pid {pid})");
                } else {
                    failed.push(format!("{label} (pid {pid})"));
                }
            }
        }
    }
    if failed.is_empty() {
        Ok(left_alone)
    } else {
        Err(format!("could not stop: {}", failed.join(", ")))
    }
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
    let left_alone = stop_recorded(&state, &process_start, &kill)?;
    std::fs::remove_file(&path).map_err(|e| format!("removing up.json: {e}"))?;
    if left_alone == 0 {
        println!("down: everything `up` started is stopped");
    } else {
        println!(
            "down: {left_alone} recorded process(es) were not ours any more and were left alone"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_port_something_listens_on_is_reported() {
        // Why: a live node on a service port would take the devnet's traffic, so `up` must see it.
        let held = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = held.local_addr().unwrap().to_string();
        assert_eq!(ports_in_use(&[addr.as_str()]), vec![addr]);
    }

    #[test]
    fn a_free_port_is_not_reported() {
        let addr = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            probe.local_addr().unwrap().to_string()
        };
        assert!(ports_in_use(&[addr.as_str()]).is_empty());
    }

    fn one_service(pid: u64, started: &str) -> Value {
        json!({ "services": [{ "label": "validator-0", "pid": pid, "started": started }] })
    }

    #[test]
    fn down_with_a_reused_pid_kills_nothing() {
        // Why: after a reboot or PID reuse, a foreign process (possibly the live seed) holds the number.
        let state = one_service(4242, "100");
        let killed = std::cell::RefCell::new(Vec::new());
        let foreign = |_pid: u32| Some("999".to_owned());
        let kill = |pid: u32| {
            killed.borrow_mut().push(pid);
            true
        };
        let left = stop_recorded(&state, &foreign, &kill).unwrap();
        assert!(killed.borrow().is_empty());
        assert_eq!(left, 1);
    }

    #[test]
    fn down_with_no_recorded_start_time_kills_nothing() {
        // Why: an up.json written before start times were recorded cannot prove ownership.
        let state = json!({ "services": [{ "label": "old", "pid": 7 }] });
        let killed = std::cell::RefCell::new(Vec::new());
        let running = |_pid: u32| Some("100".to_owned());
        let kill = |pid: u32| {
            killed.borrow_mut().push(pid);
            true
        };
        assert_eq!(stop_recorded(&state, &running, &kill).unwrap(), 1);
        assert!(killed.borrow().is_empty());
    }

    #[test]
    fn down_stops_a_recorded_process_whose_start_time_matches() {
        let state = one_service(4242, "100");
        let killed = std::cell::RefCell::new(Vec::new());
        let ours = |_pid: u32| Some("100".to_owned());
        let kill = |pid: u32| {
            killed.borrow_mut().push(pid);
            true
        };
        assert_eq!(stop_recorded(&state, &ours, &kill).unwrap(), 0);
        assert_eq!(*killed.borrow(), vec![4242]);
    }

    #[test]
    fn down_does_nothing_to_a_pid_that_has_exited() {
        let state = one_service(4242, "100");
        let killed = std::cell::RefCell::new(Vec::new());
        let gone = |_pid: u32| None;
        let kill = |pid: u32| {
            killed.borrow_mut().push(pid);
            true
        };
        assert_eq!(stop_recorded(&state, &gone, &kill).unwrap(), 0);
        assert!(killed.borrow().is_empty());
    }

    #[test]
    fn the_action_table_stops_only_on_an_exact_start_time_match() {
        assert_eq!(action(Some("1"), Some("1")), Action::Stop);
        assert_eq!(action(Some("1"), Some("2")), Action::Reused);
        assert_eq!(action(None, Some("1")), Action::Reused);
        assert_eq!(action(Some("1"), None), Action::Gone);
    }
}
