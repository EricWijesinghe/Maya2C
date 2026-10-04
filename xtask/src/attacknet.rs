//! `cargo xtask attacknet` — a self-run adversarial testnet: seven
//! `maya2c-node` validators as separate processes on this machine, attacked
//! one way after another while the honest ones are checked for agreement.
//!
//! Each attack states what must hold, and a failure stops the run:
//!
//! 1. **crash f** — two of seven stop; the other five (n − f) keep
//!    committing on one chain;
//! 2. **crash f + 1** — three stop; the chain must *halt*, never fork, and
//!    resume on one chain once they return (safety over liveness);
//! 3. **stolen key** — a second process signs with validator 0's key, so
//!    validator 0 equivocates; the honest nodes must not fork;
//! 4. **garbage on the wire** — hundreds of TCP connections to every p2p
//!    port sending random bytes; consensus must not notice;
//! 5. **RPC flood** — malformed JSON, oversized bodies and invalid
//!    transactions at one node's RPC; it must stay up and refuse them;
//! 6. **long outage** — one validator down for longer than the engine keeps
//!    rounds, then restarted with `--catch-up-from`; it must rejoin.
//!
//! `--rounds N` repeats the whole sequence; `--weighted` runs a
//! stake-weighted genesis (ADR-040). Results go to stdout and to
//! `reports/attacknet/<date>.md`.
//!
//! What this is not: an incentivised testnet, independent operators, or an
//! audit. Every process here is one operator's, on one machine, over
//! loopback. It finds the bugs a single operator can find by attacking their
//! own network; it is a floor, not a substitute.

use std::io::Write as _;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::devnet::{self, Procs, Result};

/// n = 7 tolerates f = 2.
const VALIDATORS: u16 = 7;
const FAULTS: u16 = 2;
/// node 0 shares the wallet's RPC port, so `devnet::send` reaches it.
const RPC_BASE: u16 = devnet::RPC_PORT;
/// Clear of the live testnet seed (31100/31101) and its peers (312xx).
const P2P_BASE: u16 = 32_300;
/// The stolen-key twin of validator 0.
const TWIN: u16 = VALIDATORS;
const GENESIS_BALANCE: u64 = 10_000_000;
const BOND: u64 = 100_000;
const WAIT: Duration = Duration::from_secs(180);
const POLL: Duration = Duration::from_millis(500);
/// How long a halted chain is watched to prove it stays halted.
const HALT_WATCH: Duration = Duration::from_secs(20);
/// Longer than the engine's in-memory round window, so a rejoin needs
/// attested catch-up rather than gossip (ADR-038).
const LONG_OUTAGE: Duration = Duration::from_secs(45);
const FLOOD_FOR: Duration = Duration::from_secs(15);
const GARBAGE_CONNECTIONS: usize = 200;

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

fn block_id(i: u16, h: u64) -> Option<String> {
    let b = rpc(i, "get_block_by_height", &json!([h])).ok()?;
    b["header"]["id"].as_str().map(str::to_owned)
}

/// The nodes agree on the block at the lowest tip among them, which is at
/// least `min`. A node that does not answer counts as disagreeing.
fn agree(nodes: &[u16], min: u64) -> bool {
    let Some(low) = nodes.iter().map(|i| height(*i)).collect::<Option<Vec<_>>>() else {
        return false;
    };
    let low = low.into_iter().min().unwrap_or(0);
    if low < min {
        return false;
    }
    let ids: Option<Vec<String>> = nodes.iter().map(|i| block_id(*i, low)).collect();
    ids.is_some_and(|ids| ids.iter().all(|id| *id == ids[0]))
}

/// Every pair of honest nodes agrees at every height both have: a fork
/// anywhere below the tip fails this even if the tips later converge.
fn no_fork(nodes: &[u16]) -> Result<u64> {
    let low = nodes
        .iter()
        .filter_map(|i| height(*i))
        .min()
        .ok_or("no node answers")?;
    for h in 1..=low {
        let ids: Vec<Option<String>> = nodes.iter().map(|i| block_id(*i, h)).collect();
        if ids.iter().any(Option::is_none) || ids.iter().any(|id| *id != ids[0]) {
            return Err(format!("FORK at height {h}: {ids:?}"));
        }
    }
    Ok(low)
}

fn wait_until(what: &str, mut check: impl FnMut() -> bool) -> Result<Duration> {
    let started = Instant::now();
    while started.elapsed() < WAIT {
        if check() {
            return Ok(started.elapsed());
        }
        std::thread::sleep(POLL);
    }
    Err(format!("{what}: not within {WAIT:?}"))
}

struct Net {
    work: PathBuf,
    /// Validator `i` is slot `i`; the stolen-key twin is slot `TWIN`.
    procs: Procs,
    /// What each attack showed, for the report.
    log: Vec<String>,
}

impl Net {
    fn say(&mut self, line: String) {
        println!("attacknet: {line}");
        self.log.push(line);
    }

    fn honest(except: &[u16]) -> Vec<u16> {
        (0..VALIDATORS).filter(|i| !except.contains(i)).collect()
    }

    fn start(&self, i: u16, catch_up: Option<u16>) -> Result<Child> {
        start_node(&self.work, i, catch_up)
    }

    fn restart(&mut self, i: u16, catch_up: Option<u16>) -> Result<()> {
        let child = self.start(i, catch_up)?;
        self.procs.replace(usize::from(i), child);
        Ok(())
    }
}

fn setup(work: &Path, weighted: bool) -> Result<()> {
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
    // The twin holds a copy of validator 0's key: the stolen-key attack.
    let twin = work.join(format!("v{TWIN}"));
    std::fs::create_dir_all(&twin).map_err(|e| format!("{}: {e}", twin.display()))?;
    std::fs::copy(
        work.join("v0").join("validator.key"),
        twin.join("validator.key"),
    )
    .map_err(|e| format!("copying the stolen key: {e}"))?;
    let generated = devnet::output(devnet::wallet(work).arg("generate"))?;
    let address =
        devnet::field(&generated, "address").ok_or("l1-wallet generate printed no address")?;
    let bonds: Vec<Value> = validators
        .iter()
        .map(|_| json!({ "operator": address, "bond": BOND }))
        .collect();
    let genesis = json!({
        "chain_id": "maya2c-attacknet",
        "timestamp": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        "difficulty_bits": 0,
        "pow_limit_bits": 0,
        "allocations": [{ "address": address, "balance": GENESIS_BALANCE }],
        "bft": {
            "validators": validators, "anchor_timeout_ms": 1000, "batch_size": 500,
            "staking": { "epoch_blocks": 60, "bonds": bonds, "stake_weighted": weighted }
        }
    });
    std::fs::write(work.join("genesis.json"), genesis.to_string())
        .map_err(|e| format!("writing genesis: {e}"))
}

fn start_node(work: &Path, i: u16, catch_up: Option<u16>) -> Result<Child> {
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
    if let Some(from) = catch_up {
        cmd.args(["--catch-up-from", &format!("http://{}", rpc_addr(from))]);
    }
    cmd.stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("starting node {i}: {e}"))
}

/// Attack 1: f validators crash; n − f carry on.
fn crash_f(net: &mut Net) -> Result<()> {
    let down: Vec<u16> = (VALIDATORS - FAULTS..VALIDATORS).collect();
    for i in &down {
        net.procs.kill(usize::from(*i));
    }
    let live = Net::honest(&down);
    let from = height(0).ok_or("node 0 silent")?;
    let t = wait_until("n - f validators committing", || agree(&live, from + 5))?;
    net.say(format!(
        "crash f={FAULTS}: {} validators committed 5 blocks in {t:.1?}",
        live.len()
    ));
    for i in down {
        net.restart(i, Some(0))?;
    }
    let all = Net::honest(&[]);
    let target = height(0).ok_or("node 0 silent")?;
    let t = wait_until("crashed validators rejoining", || agree(&all, target))?;
    net.say(format!("crash f: both rejoined in {t:.1?}"));
    Ok(())
}

/// Attack 2: f + 1 crash; the chain must stop rather than fork, then resume.
fn crash_f_plus_one(net: &mut Net) -> Result<()> {
    let down: Vec<u16> = (VALIDATORS - FAULTS - 1..VALIDATORS).collect();
    for i in &down {
        net.procs.kill(usize::from(*i));
    }
    let live = Net::honest(&down);
    // Let in-flight rounds settle, then watch for progress.
    std::thread::sleep(Duration::from_secs(5));
    let before = live
        .iter()
        .filter_map(|i| height(*i))
        .max()
        .ok_or("no node answers")?;
    std::thread::sleep(HALT_WATCH);
    let after = live
        .iter()
        .filter_map(|i| height(*i))
        .max()
        .ok_or("no node answers")?;
    if after > before + 1 {
        return Err(format!(
            "crash f+1: {} validators kept committing without a quorum ({before} -> {after})",
            live.len()
        ));
    }
    net.say(format!(
        "crash f+1={}: halted at {after} for {HALT_WATCH:?} (no quorum), as BFT must",
        FAULTS + 1
    ));
    for i in down {
        net.restart(i, Some(0))?;
    }
    let all = Net::honest(&[]);
    let t = wait_until("the halted chain resuming", || agree(&all, after + 3))?;
    net.say(format!(
        "crash f+1: resumed on one chain in {t:.1?} once quorum returned"
    ));
    Ok(())
}

/// Attack 3: a stolen key signs from a second machine. Validator 0
/// equivocates; nobody may fork.
fn stolen_key(net: &mut Net) -> Result<()> {
    let twin = net.start(TWIN, Some(1))?;
    net.procs.replace(usize::from(TWIN), twin);
    let honest = Net::honest(&[0]);
    let from = height(1).ok_or("node 1 silent")?;
    let t = wait_until("honest nodes committing beside an equivocator", || {
        agree(&honest, from + 10)
    })?;
    let checked = no_fork(&Net::honest(&[]))?;
    net.procs.kill(usize::from(TWIN));
    let notices = std::fs::read_to_string(net.work.join("v1").join("node.log"))
        .unwrap_or_default()
        .matches("equivocat")
        .count();
    net.say(format!(
        "stolen key: 10 blocks in {t:.1?} with validator 0 signed twice; no fork through height {checked}; node 1 logged {notices} equivocation notice(s)"
    ));
    Ok(())
}

/// Attack 4: random bytes at every p2p port from hundreds of connections.
fn garbage_on_the_wire(net: &mut Net) -> Result<()> {
    let from = height(0).ok_or("node 0 silent")?;
    let started = Instant::now();
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut sent = 0usize;
    let mut sockets = Vec::new();
    for n in 0..GARBAGE_CONNECTIONS {
        let port = P2P_BASE + u16::try_from(n % usize::from(VALIDATORS)).unwrap_or(0);
        if let Ok(mut s) = TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}")
                .parse()
                .map_err(|e| format!("{e}"))?,
            Duration::from_millis(500),
        ) {
            let junk: Vec<u8> = (0..4096)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    seed.to_le_bytes()[0]
                })
                .collect();
            if s.write_all(&junk).is_ok() {
                sent += junk.len();
            }
            sockets.push(s); // held open: a connection-slot exhaustion attempt
        }
    }
    let all = Net::honest(&[]);
    let t = wait_until("consensus under garbage", || agree(&all, from + 5))?;
    drop(sockets);
    net.say(format!(
        "garbage: {GARBAGE_CONNECTIONS} connections, {sent} random bytes; 5 blocks committed in {t:.1?} (attack began {:.1?} earlier)",
        started.elapsed().saturating_sub(t)
    ));
    Ok(())
}

/// Attack 5: malformed, oversized and invalid requests at node 0's RPC.
fn rpc_flood(net: &mut Net) -> Result<()> {
    let from = height(0).ok_or("node 0 silent")?;
    let addr = rpc_addr(0);
    let deadline = Instant::now() + FLOOD_FOR;
    let workers: Vec<_> = (0..8)
        .map(|w| {
            let addr = addr.clone();
            std::thread::spawn(move || {
                let mut n = 0u64;
                let mut accepted_bad_tx = 0u64;
                while Instant::now() < deadline {
                    let body = match n % 4 {
                        0 => json!({"jsonrpc": "2.0", "id": n, "method": "send_raw_transaction", "params": ["00ff".repeat(64)]}),
                        1 => json!({"jsonrpc": "2.0", "id": n, "method": "no_such_method", "params": []}),
                        2 => json!({"jsonrpc": "2.0", "id": n, "method": "get_block_by_height", "params": [u64::MAX]}),
                        _ => json!({"jsonrpc": "2.0", "id": n, "method": "send_raw_transaction", "params": ["ab".repeat(600_000 + w)]}),
                    };
                    if let Ok((_, reply)) = devnet::post(&addr, "/", &body)
                        && body["method"] == "send_raw_transaction"
                        && reply.get("error").is_none()
                    {
                        accepted_bad_tx += 1;
                    }
                    n += 1;
                }
                (n, accepted_bad_tx)
            })
        })
        .collect();
    let mut total = 0;
    let mut accepted = 0;
    for w in workers {
        let (n, a) = w.join().map_err(|_| "flood worker panicked")?;
        total += n;
        accepted += a;
    }
    if accepted > 0 {
        return Err(format!(
            "rpc flood: {accepted} garbage transactions were accepted"
        ));
    }
    let all = Net::honest(&[]);
    let t = wait_until("node 0 alive after the flood", || agree(&all, from + 3))?;
    net.say(format!(
        "rpc flood: {total} bad requests in {FLOOD_FOR:?}, none accepted; node 0 answered and the chain moved on ({t:.1?})"
    ));
    Ok(())
}

/// Attack 6: an outage past the round window, then attested catch-up.
fn long_outage(net: &mut Net) -> Result<()> {
    let victim = VALIDATORS - 1;
    net.procs.kill(usize::from(victim));
    std::thread::sleep(LONG_OUTAGE);
    net.restart(victim, Some(0))?;
    let all = Net::honest(&[]);
    let target = height(0).ok_or("node 0 silent")?;
    let t = wait_until("rejoin after a long outage", || agree(&all, target))?;
    net.say(format!(
        "long outage: validator {victim} down {LONG_OUTAGE:?}, rejoined at {target} in {t:.1?}"
    ));
    Ok(())
}

fn round(net: &mut Net, n: usize) -> Result<()> {
    net.say(format!("--- round {n} ---"));
    crash_f(net)?;
    crash_f_plus_one(net)?;
    stolen_key(net)?;
    garbage_on_the_wire(net)?;
    rpc_flood(net)?;
    long_outage(net)?;
    let h = no_fork(&Net::honest(&[]))?;
    net.say(format!("round {n}: no fork anywhere through height {h}"));
    Ok(())
}

fn write_report(net: &Net, weighted: bool, outcome: &Result<()>) -> Result<PathBuf> {
    let dir = devnet::root().join("reports").join("attacknet");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = devnet::output(Command::new("git").args(["rev-parse", "--short", "HEAD"]))
        .unwrap_or_default();
    let path = dir.join(format!(
        "{}.md",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    ));
    let verdict = match outcome {
        Ok(()) => "PASS".to_owned(),
        Err(e) => format!("FAIL: {e}"),
    };
    let body = format!(
        "# Attacknet run\n\n- commit: {}\n- validators: {VALIDATORS} (f = {FAULTS}), {} genesis\n- host: {} / {}\n- verdict: **{verdict}**\n\nOne operator, one machine, loopback links: a floor, not an independent test.\n\n```\n{}\n```\n",
        stamp.trim(),
        if weighted {
            "stake-weighted"
        } else {
            "equal-weight"
        },
        std::env::consts::OS,
        std::env::consts::ARCH,
        net.log.join("\n")
    );
    std::fs::write(&path, body).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// Runs the attacknet in `target/attacknet`.
///
/// # Errors
///
/// The first attack whose guarantee did not hold, with logs left in place.
pub fn attacknet(args: &[String]) -> Result<()> {
    let weighted = args.iter().any(|a| a == "--weighted");
    let rounds = args
        .iter()
        .position(|a| a == "--rounds")
        .and_then(|p| args.get(p + 1))
        .map_or(Ok(1), |v| {
            v.parse::<usize>().map_err(|e| format!("--rounds: {e}"))
        })?;
    devnet::build(&["maya2c-node", "l1-wallet"])?;
    let work = devnet::root().join("target").join("attacknet");
    setup(&work, weighted)?;
    let mut net = Net {
        work: work.clone(),
        procs: Procs::default(),
        log: Vec::new(),
    };
    for i in 0..VALIDATORS {
        let child = net.start(i, None)?;
        net.procs.push(child);
    }
    let all = Net::honest(&[]);
    let outcome = wait_until("seven nodes on one chain", || agree(&all, 3))
        .map(|t| {
            net.say(format!(
                "{VALIDATORS} validators on one chain {t:.1?} after start"
            ))
        })
        .and_then(|()| (1..=rounds).try_for_each(|n| round(&mut net, n)));
    let report = write_report(&net, weighted, &outcome)?;
    println!("attacknet: report {}", report.display());
    outcome.map_err(|e| format!("{e} (logs: {})", work.display()))
}
