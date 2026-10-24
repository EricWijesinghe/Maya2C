//! `cargo xtask bench-tps [--senders N] [--txs N] [--allow-debug]` — finalized
//! transfers per second on the four-validator localnet.
//!
//! What a "transaction per second" means here (Standing Order 2): a transfer
//! that was signed (ML-DSA-65 + SLH-DSA hybrid), accepted by a node's mempool,
//! ordered by DAG-BFT, executed with its state root checked, and visible in a
//! balance on a replica that never received it. The clock starts at the first
//! submission and stops when the recipient's balance on node 3 equals the sum
//! of everything accepted. Signing is done first and is not in the figure:
//! it is client work, and a wallet does it once per transaction, in parallel.
//!
//! The mix is stated in the report: single-output transfers of one base unit
//! from `N` funded senders to one recipient, one sender's nonces in order.
//! Not measured here: mixed workloads, contract calls, a network with real
//! latency, validators on separate machines. Localhost links and one machine
//! shared with whatever else runs on it; the report says which.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;

use crate::devnet::{self, Procs, Result};
use crate::localnet::{self, VALIDATORS};

/// Clear of the live testnet (31400s, 8545, 18700s) and of `localnet`/`mining-localnet`.
const PORT_OFFSET: u16 = 3_000;
const SENDER_BALANCE: u64 = 1_000_000;
const SIGN_THREADS: usize = 16;
const SUBMIT_THREADS: usize = 32;
/// No new transfer visible on the replica for this long means it is stuck
/// (`BENCH_STALL_SECS` overrides it, to see whether a stalled chain recovers).
const STALL_SECS: u64 = 90;
/// 32 bytes of 0x5a, hex.
const RECIPIENT: &str = "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a";

struct Options {
    senders: usize,
    txs: usize,
    allow_debug: bool,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut o = Options {
        senders: 32,
        txs: 25,
        allow_debug: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut num = |name: &str| -> Result<usize> {
            it.next()
                .ok_or(format!("{name} needs a value"))?
                .parse()
                .map_err(|e| format!("{name}: {e}"))
        };
        match a.as_str() {
            "--senders" => o.senders = num("--senders")?,
            "--txs" => o.txs = num("--txs")?,
            "--allow-debug" => o.allow_debug = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if o.senders == 0 || o.txs == 0 {
        return Err("--senders and --txs must be at least 1".into());
    }
    Ok(o)
}

fn wallet(keystore: &Path, node: u16) -> Command {
    let mut cmd = Command::new(devnet::bin("l1-wallet"));
    cmd.env("L1_WALLET_PASSWORD", devnet::PASSWORD)
        .arg("--keystore")
        .arg(keystore)
        .args(["--rpc-url", &format!("http://{}", localnet::rpc_addr(node))]);
    cmd
}

/// Runs `job(i)` for `0..count` on `threads` threads; the first error wins.
fn parallel<T: Send>(
    count: usize,
    threads: usize,
    job: &(dyn Fn(usize) -> Result<T> + Sync),
) -> Result<Vec<T>> {
    let next = AtomicU64::new(0);
    let out: Mutex<Vec<(usize, Result<T>)>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..threads.min(count) {
            scope.spawn(|| {
                loop {
                    let i = usize::try_from(next.fetch_add(1, Ordering::SeqCst)).unwrap_or(count);
                    if i >= count {
                        break;
                    }
                    let r = job(i);
                    out.lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push((i, r));
                }
            });
        }
    });
    let mut all = out
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    all.sort_by_key(|(i, _)| *i);
    all.into_iter().map(|(_, r)| r).collect()
}

/// A wallet per sender, kept outside the localnet dir (which setup clears).
fn make_senders(dir: &Path, n: usize) -> Result<Vec<(PathBuf, String)>> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    parallel(n, SIGN_THREADS, &|i| {
        let key = dir.join(format!("s{i}.key"));
        if key.exists() {
            std::fs::remove_file(&key).map_err(|e| e.to_string())?;
        }
        let out = devnet::output(wallet(&key, 0).arg("generate"))?;
        let address = devnet::field(&out, "address").ok_or("generate printed no address")?;
        Ok((key, address))
    })
}

/// Signs `txs` transfers per sender against node 0 (nonces `0..txs`).
fn presign(senders: &[(PathBuf, String)], txs: usize) -> Result<Vec<Vec<String>>> {
    let flat = parallel(senders.len() * txs, SIGN_THREADS, &|k| {
        let (key, _) = &senders[k / txs];
        let out = devnet::output(wallet(key, 0).args([
            "send",
            "--to",
            RECIPIENT,
            "--amount",
            "1",
            "--nonce",
            &(k % txs).to_string(),
            "--no-broadcast",
        ]))?;
        devnet::field(&out, "raw").ok_or_else(|| "send printed no raw transaction".to_string())
    })?;
    Ok(flat.chunks(txs).map(<[String]>::to_vec).collect())
}

/// Submits each sender's transactions in nonce order to node `sender % 4`.
/// Returns (accepted, first refusal).
fn submit(signed: &[Vec<String>]) -> (u64, Option<String>) {
    let accepted = AtomicU64::new(0);
    let first_error: Mutex<Option<String>> = Mutex::new(None);
    let _ = parallel(signed.len(), SUBMIT_THREADS, &|s| {
        let node = u16::try_from(s % usize::from(VALIDATORS)).unwrap_or(0);
        for raw in &signed[s] {
            let request = json!({
                "jsonrpc": "2.0", "id": 1, "method": "send_raw_transaction", "params": [raw]
            });
            let reply = devnet::post(&localnet::rpc_addr(node), "/", &request);
            match reply {
                Ok((_, v)) if v.get("error").is_none() => {
                    accepted.fetch_add(1, Ordering::SeqCst);
                }
                Ok((_, v)) => note(&first_error, format!("{}", v["error"])),
                Err(e) => note(&first_error, e),
            }
        }
        Ok(())
    });
    (
        accepted.load(Ordering::SeqCst),
        first_error
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
}

fn note(slot: &Mutex<Option<String>>, message: String) {
    let mut g = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    g.get_or_insert(message);
}

/// Waits until node 3 shows `expected` at the recipient, or stalls.
/// Returns the time it took and the balance reached.
fn await_balance(expected: u64, started: Instant) -> (Duration, u64) {
    let stall = Duration::from_secs(
        std::env::var("BENCH_STALL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(STALL_SECS),
    );
    let mut last = 0;
    let mut moved = Instant::now();
    loop {
        let now = localnet::balance(VALIDATORS - 1, RECIPIENT).unwrap_or(last);
        if now != last {
            last = now;
            moved = Instant::now();
        }
        if now >= expected || moved.elapsed() > stall {
            return (started.elapsed(), now);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn command_output(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "MISSING".to_string(), |s| s.trim().to_string())
}

fn report(
    o: &Options,
    accepted: u64,
    reached: u64,
    (took, submitted_in): (Duration, Duration),
    blocks: u64,
) -> String {
    let secs = took.as_secs_f64();
    let tps = reached as f64 / secs;
    let sub = submitted_in.as_secs_f64();
    let submit_rate = accepted as f64 / sub;
    let cores = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    format!(
        "# bench-tps (localnet, {VALIDATORS} validators on one machine)\n\n\
         - commit: {}\n- os: {} / {}\n- logical cores: {cores}\n- binaries: {}\n\
         - command: cargo xtask bench-tps --senders {} --txs {}\n\
         - mix: single-output transfers of 1 base unit, {} senders to one recipient, hybrid ML-DSA-65 + SLH-DSA\n\
         - accepted by mempools: {accepted}; finalized and visible on a replica: {reached}\n\
         - elapsed first-submit to last-visible: {secs:.2}s over {blocks} blocks\n\
         - of which submitting to the mempools took {sub:.2}s ({submit_rate:.1} accepted/s)\n\
         - **finalized transfers/s: {tps:.1}**\n\n\
         Shared machine, localhost links, other processes were running. Signing excluded.\n\
         Not measured: contract calls, mixed loads, WAN latency, separate machines.\n",
        command_output("git", &["rev-parse", "--short", "HEAD"]),
        std::env::consts::OS,
        command_output("cmd", &["/C", "ver"]),
        devnet::bin("maya2c-node").display(),
        o.senders,
        o.txs,
        o.senders,
    )
}

/// Entry point.
///
/// # Errors
///
/// Anything that stops the run: a debug build, busy ports, a node that does
/// not come up, or a transfer that is never finalized.
pub fn bench_tps(args: &[String]) -> Result<()> {
    let o = parse(args)?;
    if devnet::bin("maya2c-node")
        .to_string_lossy()
        .contains("debug")
        && !o.allow_debug
    {
        return Err(
            "debug binaries measure the wrong thing: build with `cargo build --release \
             -p maya2c-node -p l1-wallet` and set MAYA2C_BIN_DIR=target/release (or --allow-debug \
             for a smoke run)"
                .into(),
        );
    }
    devnet::build(&["maya2c-node", "l1-wallet"])?;
    localnet::use_port_offset(PORT_OFFSET);
    localnet::ports_free()?;
    let root = devnet::root().join("target");
    let senders = make_senders(&root.join("bench-tps-keys"), o.senders)?;
    let work = root.join("bench-tps");
    let addresses: Vec<String> = senders.iter().map(|(_, a)| a.clone()).collect();
    let genesis_address = localnet::setup_tweaked(&work, &|g| {
        if let Some(a) = g["allocations"].as_array_mut() {
            a.extend(
                addresses
                    .iter()
                    .map(|address| json!({ "address": address, "balance": SENDER_BALANCE })),
            );
        }
    })?;
    std::fs::write(work.join("node.toml"), "[rpc]\nrate_limit_per_second = 0\n")
        .map_err(|e| format!("node.toml: {e}"))?;
    let mut procs = Procs::default();
    for i in 0..VALIDATORS {
        procs.push(localnet::start(&work, i)?);
    }
    let all: Vec<u16> = (0..VALIDATORS).collect();
    localnet::wait_until("four nodes on one chain", || {
        localnet::agree(&all, &genesis_address, 3)
    })?;
    println!("bench-tps: signing {} transfers", o.senders * o.txs);
    let signed = presign(&senders, o.txs)?;
    let before = localnet::tip(0, &genesis_address)?.0;
    let started = Instant::now();
    let (accepted, refusal) = submit(&signed);
    let submitted_in = started.elapsed();
    if let Some(e) = &refusal {
        println!("bench-tps: first refusal: {e}");
    }
    let (took, reached) = await_balance(accepted, started);
    let after = localnet::tip(0, &genesis_address)?.0;
    let text = report(
        &o,
        accepted,
        reached,
        (took, submitted_in),
        after.saturating_sub(before),
    );
    println!("{text}");
    let dir = devnet::root().join("reports").join("bench");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    std::fs::write(dir.join(format!("tps-{stamp}.md")), &text).map_err(|e| e.to_string())?;
    if reached < accepted || accepted == 0 {
        return Err(format!(
            "only {reached} of {accepted} accepted transfers became visible on node 3"
        ));
    }
    Ok(())
}
