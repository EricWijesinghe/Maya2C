//! `cargo xtask bft-devnet` — a four-validator DAG-BFT devnet on localhost,
//! run by the real binaries: four validators and one observer as processes,
//! transfers sent over JSON-RPC, and what was measured printed as JSON.
//!
//! Ported from `scripts/bft_devnet.py`. It measures:
//!
//! - every node's height over time (blocks per second);
//! - time from `send` to the transfer being visible on every node;
//! - whether every node agrees on the block id at the common height.
//!
//! Nothing here is a benchmark of the chain's ceiling: five processes on one
//! machine, dev profile unless `--release`, localhost links. It proves the
//! binary runs DAG-BFT end to end and prints the numbers with the hardware
//! they came from.
//!
//! Exits non-zero when a check fails, after printing the report.

use std::fs::File;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::bench_tps::command_output;
use crate::devnet::{self, Procs, Result};
use crate::ordered_json::Json;

const VALIDATORS: u16 = 4;
/// The four validators and one observer.
const NODES: u16 = VALIDATORS + 1;
const P2P_BASE: u16 = 31_000;
const RPC_BASE: u16 = devnet::RPC_PORT;
/// Node 0's metrics exporter.
const METRICS_PORT: u16 = 33_000;
const GENESIS_BALANCE: u64 = 10_000_000;
const BOND: u64 = 100_000;
/// Short epochs, so one run crosses several.
const EPOCH_BLOCKS: u64 = 20;
const AMOUNT: u64 = 100;
const SETTLE_WAIT: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_millis(100);
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

/// Command-line options, with the defaults the Python script had.
struct Options {
    workdir: PathBuf,
    seconds: u64,
    transfers: u64,
    release: bool,
}

/// What `measure` observed.
struct Measured {
    elapsed: f64,
    heights: Vec<u64>,
    blocks_per_s: Vec<f64>,
    latencies: Vec<Option<f64>>,
    agree: bool,
    common: u64,
    balances: Vec<u64>,
    metrics: Json,
}

impl Measured {
    fn passed(&self) -> bool {
        self.agree
            && self.common > 0
            && self.latencies.iter().all(Option::is_some)
            && self.balances.windows(2).all(|w| w[0] == w[1])
    }
}

fn parse(args: &[String]) -> Result<Options> {
    let mut o = Options {
        workdir: PathBuf::from("D:/Temp/maya-bft-devnet"),
        seconds: 60,
        transfers: 5,
        release: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(f, v)| (f, Some(v.to_owned())));
        match flag {
            "--release" => o.release = true,
            "--workdir" => o.workdir = PathBuf::from(take(flag, inline, &mut it)?),
            "--seconds" => {
                o.seconds = take(flag, inline, &mut it)?
                    .parse()
                    .map_err(|e| format!("--seconds: {e}"))?;
            }
            "--transfers" => {
                o.transfers = take(flag, inline, &mut it)?
                    .parse()
                    .map_err(|e| format!("--transfers: {e}"))?;
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(o)
}

/// The value of `flag`, from `--flag=value` or the next argument.
fn take(
    flag: &str,
    inline: Option<String>,
    it: &mut std::slice::Iter<'_, String>,
) -> Result<String> {
    inline
        .or_else(|| it.next().cloned())
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Builds the three binaries and returns the directory holding them.
fn build(release: bool) -> Result<PathBuf> {
    let profile = if release { "release" } else { "debug" };
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.arg("build");
    if release {
        cmd.arg("--release");
    }
    cmd.args([
        "-p",
        "maya2c-node",
        "-p",
        "l1-wallet",
        "-p",
        "maya2c-peerid",
    ])
    .current_dir(devnet::root())
    .stdout(Stdio::null());
    println!(
        "building: cargo build{} -p maya2c-node -p l1-wallet -p maya2c-peerid",
        if release { " --release" } else { "" }
    );
    let status = cmd.status().map_err(|e| format!("cargo build: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("cargo build failed: {status}"))?;
    Ok(devnet::root().join("target").join(profile))
}

fn bin(bins: &Path, name: &str) -> PathBuf {
    bins.join(format!("{name}{EXE}"))
}

fn node_dir(work: &Path, i: u16) -> PathBuf {
    if i < VALIDATORS {
        work.join(format!("v{i}"))
    } else {
        work.join("observer")
    }
}

/// `l1-wallet` on this run's keystore.
fn wallet(bins: &Path, work: &Path) -> Command {
    let mut cmd = Command::new(bin(bins, "l1-wallet"));
    cmd.env("L1_WALLET_PASSWORD", devnet::PASSWORD)
        .arg("--keystore")
        .arg(work.join("wallet.key"));
    cmd
}

fn recipient() -> String {
    "77".repeat(32)
}

fn genesis(pubkeys: &[String], address: &str) -> Value {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let bonds: Vec<Value> = pubkeys
        .iter()
        .map(|_| json!({ "operator": address, "bond": BOND }))
        .collect();
    json!({
        "chain_id": "maya-bft-devnet",
        "timestamp": timestamp,
        "difficulty_bits": 0,
        "pow_limit_bits": 0,
        "allocations": [{ "address": address, "balance": GENESIS_BALANCE }],
        "bft": {
            "validators": pubkeys,
            "anchor_timeout_ms": 1000,
            "batch_size": 500,
            // ADR-029: fees live; the wallet adds the fee output itself.
            "fees": { "initial_base_fee": 1, "min_base_fee": 1,
                      "target_block_bytes": 2_621_440, "change_denominator": 8 },
            // ADR-028: the validators bonded at genesis by the wallet's address.
            "staking": { "epoch_blocks": EPOCH_BLOCKS, "bonds": bonds }
        }
    })
}

/// A fresh run directory: validator keys, a funded wallet, the genesis and
/// one peer id per node. Returns the peer ids.
fn setup(bins: &Path, work: &Path) -> Result<Vec<String>> {
    if work.exists() {
        std::fs::remove_dir_all(work).map_err(|e| format!("clearing {}: {e}", work.display()))?;
    }
    std::fs::create_dir_all(work).map_err(|e| format!("creating {}: {e}", work.display()))?;
    let mut pubkeys = Vec::new();
    for i in 0..VALIDATORS {
        let d = node_dir(work, i);
        std::fs::create_dir_all(&d).map_err(|e| format!("creating {}: {e}", d.display()))?;
        let key = devnet::output(
            Command::new(bin(bins, "maya2c-node"))
                .arg("--generate-validator-key")
                .arg(d.join("validator.key")),
        )?;
        pubkeys.push(key.trim().to_owned());
    }
    let generated = devnet::output(wallet(bins, work).arg("generate"))?;
    let address =
        devnet::field(&generated, "address").ok_or("l1-wallet generate printed no address")?;
    let doc =
        serde_json::to_string_pretty(&genesis(&pubkeys, &address)).map_err(|e| e.to_string())?;
    std::fs::write(work.join("genesis.json"), doc).map_err(|e| format!("writing genesis: {e}"))?;
    let mut peers = Vec::new();
    for i in 0..NODES {
        let d = node_dir(work, i);
        std::fs::create_dir_all(&d).map_err(|e| format!("creating {}: {e}", d.display()))?;
        let out = devnet::output(
            Command::new(bin(bins, "maya2c-peerid"))
                .arg("--data-dir")
                .arg(&d)
                .arg("--create"),
        )?;
        peers.push(
            out.split_whitespace()
                .last()
                .ok_or("maya2c-peerid printed no peer id")?
                .to_owned(),
        );
    }
    Ok(peers)
}

/// Refuses to start on ports something already holds: a foreign node there
/// would answer for ours (the 2026-10-12 incident in `localnet.rs`).
fn ports_free() -> Result<()> {
    let mut ports: Vec<u16> = (0..NODES)
        .flat_map(|i| [RPC_BASE + i, P2P_BASE + i])
        .collect();
    ports.push(METRICS_PORT);
    for port in ports {
        TcpListener::bind(("127.0.0.1", port)).map_err(|e| {
            format!("port {port} is in use ({e}); is a live node running on this machine?")
        })?;
    }
    Ok(())
}

/// Starts all five nodes, each logging to `<node dir>/node.log`.
fn start(bins: &Path, work: &Path, peers: &[String]) -> Result<Procs> {
    let mut procs = Procs::default();
    for i in 0..NODES {
        procs.push(spawn_node(bins, work, peers, i)?);
    }
    Ok(procs)
}

fn spawn_node(bins: &Path, work: &Path, peers: &[String], i: u16) -> Result<Child> {
    let d = node_dir(work, i);
    let mut cmd = Command::new(bin(bins, "maya2c-node"));
    cmd.arg("--genesis")
        .arg(work.join("genesis.json"))
        .arg("--data-dir")
        .arg(&d)
        .arg("--rpc-addr")
        .arg(format!("127.0.0.1:{}", RPC_BASE + i))
        .arg("--p2p-port")
        .arg((P2P_BASE + i).to_string())
        // Every node is on 127.0.0.1.
        .arg("--allow-private-peers");
    for j in (0..NODES).filter(|&j| j != i) {
        let peer = &peers[usize::from(j)];
        cmd.arg("--bootnode")
            .arg(format!("/ip4/127.0.0.1/tcp/{}/p2p/{peer}", P2P_BASE + j));
    }
    if i < VALIDATORS {
        cmd.arg("--validator-key").arg(d.join("validator.key"));
    }
    if i == 0 {
        cmd.arg("--metrics-addr")
            .arg(format!("127.0.0.1:{METRICS_PORT}"));
    }
    let log = File::create(d.join("node.log")).map_err(|e| format!("node {i} log: {e}"))?;
    let stderr = log.try_clone().map_err(|e| format!("node {i} log: {e}"))?;
    cmd.stdout(log).stderr(stderr);
    cmd.spawn().map_err(|e| format!("starting node {i}: {e}"))
}

fn rpc_ports() -> impl Iterator<Item = u16> {
    (0..NODES).map(|i| RPC_BASE + i)
}

/// A JSON-RPC call to the node on `port`. HTTP errors, JSON-RPC errors and a
/// missing result are all errors.
fn rpc_at(port: u16, method: &str, params: &Value) -> Result<Value> {
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let (status, reply) = devnet::post(&format!("127.0.0.1:{port}"), "/", &request)?;
    if status >= 400 {
        return Err(format!("port {port} {method}: HTTP {status}"));
    }
    if let Some(error) = reply.get("error") {
        return Err(format!("port {port} {method}: {error}"));
    }
    reply
        .get("result")
        .cloned()
        .ok_or_else(|| format!("port {port} {method}: no result"))
}

/// Highest height with a block, searched up from `hint`. Any error ends the
/// walk: "no block at this height" is the normal end.
fn height(port: u16, hint: u64) -> u64 {
    let mut h = hint;
    while rpc_at(port, "get_block_by_height", &json!([h + 1])).is_ok() {
        h += 1;
    }
    h
}

/// Python's truthiness, for `b.get("id") or b.get("hash")`.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn block_id(port: u16, h: u64) -> Result<String> {
    let b = rpc_at(port, "get_block_by_height", &json!([h]))?;
    let id = ["id", "hash"]
        .iter()
        .filter_map(|key| b.get(*key))
        .find(|v| truthy(v))
        .map_or_else(
            || b.to_string(),
            |v| v.as_str().map_or_else(|| v.to_string(), str::to_owned),
        );
    Ok(id)
}

/// The recipient's balance on `port`. Amounts are decimal strings since
/// ADR-046; a JSON number is read too.
fn balance(port: u16) -> Result<u64> {
    let v = rpc_at(port, "get_balance", &json!([recipient()]))?;
    let field = v.get("balance").ok_or("get_balance carries no balance")?;
    field
        .as_str()
        .map_or_else(|| field.as_u64(), |s| s.parse().ok())
        .ok_or_else(|| format!("port {port}: unreadable balance {field}"))
}

fn balances() -> Result<Vec<u64>> {
    rpc_ports().map(balance).collect()
}

/// Sends `AMOUNT` to the recipient through node `port`, with nonce `nonce`.
/// The wallet signs only for the genesis it is told to expect (ADR-036).
fn send(bins: &Path, work: &Path, port: u16, nonce: u64) -> Result<()> {
    let genesis = devnet::genesis_id()?;
    devnet::output(wallet(bins, work).args([
        "--rpc-url",
        &format!("http://127.0.0.1:{port}"),
        "send",
        "--yes",
        "--genesis",
        &genesis,
        "--to",
        &recipient(),
        "--amount",
        &AMOUNT.to_string(),
        "--nonce",
        &nonce.to_string(),
    ]))?;
    Ok(())
}

/// Seconds from `sent` until every node shows at least `expected` for the
/// recipient, or `None` after `SETTLE_WAIT`.
fn settle(sent: Instant, expected: u64) -> Result<Option<f64>> {
    while sent.elapsed() < SETTLE_WAIT {
        if balances()?.iter().all(|&b| b >= expected) {
            return Ok(Some(round(sent.elapsed().as_secs_f64(), 3)));
        }
        std::thread::sleep(POLL);
    }
    Ok(None)
}

fn run_transfers(bins: &Path, work: &Path, count: u64) -> Result<Vec<Option<f64>>> {
    let mut latencies = Vec::new();
    for n in 0..count {
        let port =
            RPC_BASE + u16::try_from(n % u64::from(VALIDATORS)).map_err(|e| e.to_string())?;
        let sent = Instant::now();
        send(bins, work, port, n)?;
        latencies.push(settle(sent, AMOUNT * (n + 1))?);
    }
    Ok(latencies)
}

/// One HTTP GET on 127.0.0.1; the body of a 2xx reply.
fn http_get(port: u16, path: &str) -> Result<String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect(&addr).map_err(|e| format!("{addr}: {e}"))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|e| e.to_string())?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| format!("{addr}: {e}"))?;
    let mut raw = String::new();
    stream
        .read_to_string(&mut raw)
        .map_err(|e| format!("{addr}: {e}"))?;
    let (head, body) = raw
        .split_once("\r\n\r\n")
        .ok_or("malformed HTTP response")?;
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or("no HTTP status")?;
    if !(200..300).contains(&status) {
        return Err(format!("{addr}{path}: HTTP {status}"));
    }
    Ok(body.to_owned())
}

/// A height as `f64`. Heights stay far below 2^52, so the conversion is exact.
#[allow(clippy::cast_precision_loss)]
fn signed(h: u64) -> f64 {
    h as f64
}

/// Python's `round(x, places)`: the decimal rounding of the binary value.
fn round(x: f64, places: usize) -> f64 {
    format!("{x:.places$}").parse().unwrap_or(x)
}

/// A JSON number. Non-finite values have no JSON form and print as null.
fn num(x: f64) -> Json {
    serde_json::Number::from_f64(x).map_or(Json::Null, Json::Number)
}

fn last_f64(line: &str) -> Result<f64> {
    line.split_whitespace()
        .last()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("no number in metric line: {line}"))
}

/// Node 0's exporter: the finality and RPC metrics, with their values.
fn scrape() -> Result<Json> {
    parse_metrics(&http_get(METRICS_PORT, "/metrics")?)
}

/// The Prometheus text the node serves, reduced to the figures the report
/// carries. Quantiles are bucket upper bounds, as the histogram gives them.
fn parse_metrics(text: &str) -> Result<Json> {
    let mut buckets: Vec<(f64, f64)> = Vec::new();
    let (mut count, mut total, mut rpc_ok, mut rpc_err) = (0.0, 0.0, 0.0, 0.0);
    let mut out = Json::Object(Vec::new());
    for line in text.lines() {
        if line.starts_with("maya_finality_latency_seconds_bucket") {
            let le = line
                .split("le=\"")
                .nth(1)
                .and_then(|s| s.split('"').next())
                .ok_or_else(|| format!("no le in {line}"))?;
            let le = if le == "+Inf" {
                f64::INFINITY
            } else {
                le.parse().map_err(|e| format!("le {le}: {e}"))?
            };
            buckets.push((le, last_f64(line)?));
        } else if line.starts_with("maya_finality_latency_seconds_count") {
            count = last_f64(line)?;
        } else if line.starts_with("maya_finality_latency_seconds_sum") {
            total = last_f64(line)?;
        } else if line.starts_with("maya_rpc_requests_total") {
            if line.contains("outcome=\"ok\"") {
                rpc_ok += last_f64(line)?;
            } else {
                rpc_err += last_f64(line)?;
            }
        } else if line.starts_with("maya_sync_duration_seconds ")
            || line.starts_with("maya_blocks_imported_total ")
        {
            let name = line.split_whitespace().next().unwrap_or_default();
            out.set(name, num(last_f64(line)?))?;
        }
    }
    let quantile = |q: f64| {
        buckets
            .iter()
            .find(|(_, c)| count > 0.0 && *c >= q * count)
            .map_or(Json::Null, |(le, _)| num(*le))
    };
    let mean = if count > 0.0 {
        num(round(total / count, 3))
    } else {
        Json::Null
    };
    for (key, value) in [
        ("finality_observations", num(count)),
        ("finality_mean_s", mean),
        ("finality_p50_bucket_le_s", quantile(0.5)),
        ("finality_p99_bucket_le_s", quantile(0.99)),
        ("rpc_calls_ok", num(rpc_ok)),
        ("rpc_calls_error", num(rpc_err)),
    ] {
        out.set(key, value)?;
    }
    Ok(out)
}

/// Waits for every RPC port, then runs the timed part of the devnet.
fn wait_and_measure(bins: &Path, o: &Options) -> Result<Measured> {
    for port in rpc_ports() {
        devnet::wait_for_http(&format!("127.0.0.1:{port}"))?;
    }
    measure(bins, o)
}

fn measure(bins: &Path, o: &Options) -> Result<Measured> {
    let t0 = Instant::now();
    let h0: Vec<u64> = rpc_ports().map(|p| height(p, 0)).collect();
    let latencies = run_transfers(bins, &o.workdir, o.transfers)?;
    while t0.elapsed() < Duration::from_secs(o.seconds) {
        std::thread::sleep(Duration::from_secs(1));
    }
    let elapsed = t0.elapsed().as_secs_f64();
    let heights: Vec<u64> = rpc_ports()
        .zip(h0.iter().copied())
        .map(|(p, h)| height(p, h))
        .collect();
    let common = heights.iter().copied().min().unwrap_or(0);
    let ids = rpc_ports()
        .map(|p| block_id(p, common))
        .collect::<Result<Vec<_>>>()?;
    let balances = balances()?;
    let metrics = scrape()?;
    let blocks_per_s = h0
        .iter()
        .zip(&heights)
        .map(|(a, b)| round((signed(*b) - signed(*a)) / elapsed, 2))
        .collect();
    Ok(Measured {
        elapsed,
        heights,
        blocks_per_s,
        latencies,
        agree: ids.iter().all(|id| *id == ids[0]),
        common,
        balances,
        metrics,
    })
}

fn obj(pairs: Vec<(&str, Json)>) -> Json {
    Json::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn uints(xs: &[u64]) -> Json {
    Json::Array(xs.iter().map(|&x| Json::uint(x)).collect())
}

fn floats(xs: &[f64]) -> Json {
    Json::Array(xs.iter().map(|&x| num(x)).collect())
}

fn os_release() -> String {
    if cfg!(windows) {
        command_output("cmd", &["/C", "ver"])
    } else {
        command_output("uname", &["-sr"])
    }
}

fn report(o: &Options, m: &Measured) -> Json {
    let cpus = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    let latencies = Json::Array(
        m.latencies
            .iter()
            .map(|l| l.map_or(Json::Null, num))
            .collect(),
    );
    obj(vec![
        (
            "hardware",
            obj(vec![
                ("machine", Json::String(std::env::consts::ARCH.into())),
                (
                    "processor",
                    Json::String(std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default()),
                ),
                ("system", Json::String(os_release())),
                ("cpus", Json::uint(cpus as u64)),
            ]),
        ),
        (
            "profile",
            Json::String(if o.release { "release" } else { "dev" }.into()),
        ),
        (
            "commit",
            Json::String(command_output("git", &["rev-parse", "--short", "HEAD"])),
        ),
        ("metrics_node0", m.metrics.clone()),
        ("elapsed_s", num(round(m.elapsed, 1))),
        ("heights", uints(&m.heights)),
        ("blocks_per_s", floats(&m.blocks_per_s)),
        ("transfer_visible_on_all_nodes_s", latencies),
        ("agree_at_common_height", Json::Bool(m.agree)),
        ("common_height", Json::uint(m.common)),
        ("recipient_balance_per_node", uints(&m.balances)),
    ])
}

/// `cargo xtask bft-devnet [--workdir DIR] [--seconds N] [--transfers N] [--release]`
///
/// # Errors
///
/// A failed build, busy ports, a node that does not answer within a minute,
/// a refused transfer, or a failed check (printed in the report first).
pub fn run(args: &[String]) -> Result<()> {
    let o = parse(args)?;
    let bins = build(o.release)?;
    ports_free()?;
    let peers = setup(&bins, &o.workdir)?;
    let procs = start(&bins, &o.workdir, &peers)?;
    let measured = wait_and_measure(&bins, &o);
    // Children are killed here, before the report, as the Python `finally` did.
    drop(procs);
    let m = measured?;
    println!(
        "{}",
        serde_json::to_string_pretty(&report(&o, &m)).map_err(|e| e.to_string())?
    );
    if m.passed() {
        Ok(())
    } else {
        Err("bft-devnet: a check failed (see the report above)".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn flags_take_a_separate_or_inline_value() -> Result<()> {
        let o = parse(&args(&["--seconds", "7", "--transfers=2", "--release"]))?;
        assert_eq!((o.seconds, o.transfers, o.release), (7, 2, true));
        assert!(parse(&args(&["--nope"])).is_err());
        assert!(parse(&args(&["--seconds"])).is_err());
        Ok(())
    }

    #[test]
    fn metrics_become_the_report_figures() -> Result<()> {
        let text = "maya_finality_latency_seconds_bucket{le=\"0.5\"} 2\n\
            maya_finality_latency_seconds_bucket{le=\"+Inf\"} 4\n\
            maya_finality_latency_seconds_sum 3\n\
            maya_finality_latency_seconds_count 4\n\
            maya_rpc_requests_total{outcome=\"ok\"} 10\n\
            maya_rpc_requests_total{outcome=\"error\"} 1\n\
            maya_blocks_imported_total 7\n";
        let doc = parse_metrics(text)?;
        assert_eq!(doc.get("finality_observations"), Some(&num(4.0)));
        assert_eq!(doc.get("finality_mean_s"), Some(&num(0.75)));
        assert_eq!(doc.get("finality_p50_bucket_le_s"), Some(&num(0.5)));
        assert_eq!(doc.get("finality_p99_bucket_le_s"), Some(&Json::Null));
        assert_eq!(doc.get("rpc_calls_error"), Some(&num(1.0)));
        // Keyed by the full metric name, as the Python dict was.
        let blocks = doc
            .get("maya_blocks_imported_total")
            .ok_or("no maya_blocks_imported_total")?;
        assert_eq!(blocks, &num(7.0));
        Ok(())
    }

    #[test]
    fn round_matches_python_round() {
        assert!((round(0.25, 1) - 0.2).abs() < f64::EPSILON);
        assert!((round(1.23456, 3) - 1.235).abs() < f64::EPSILON);
    }
}
