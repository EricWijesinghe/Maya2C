//! `cargo xtask bench-finality [--senders N] [--txs N] [--rate R] [--allow-debug]`
//! — per-transaction latency on the four-validator localnet.
//!
//! Two clocks start at the instant a transfer is posted to a node's
//! `send_raw_transaction`, at a stated open-loop rate (the schedule does not
//! wait for earlier replies):
//! - **included**: node 3 (never the only submitter) serves a block that
//!   carries the txid. Seen by polling `get_chain_info` and
//!   `get_block_by_height`, so resolution is the poll interval.
//! - **finalized**: node 3's `get_checkpoint` reports a quorum-attested
//!   (2f+1, ADR-038) block at or above the including block's height.
//!
//! Nothing in the node changes: both signals are RPC it already serves.
//! The report carries the mix, rate, commit, hardware and command
//! (CLAUDE.md production standing orders). Not tuned toward any target.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::bench_tps::{
    PORT_OFFSET, SENDER_BALANCE, SUBMIT_THREADS, command_output, make_senders, parallel, presign,
};
use crate::devnet::{self, Procs, Result};
use crate::localnet::{self, VALIDATORS};

const POLL: Duration = Duration::from_millis(10);
/// No new finalization for this long after submission ends: give up and report what exists.
const STALL: Duration = Duration::from_secs(60);
const OBSERVER: u16 = VALIDATORS - 1;
const SECS_PER_DAY: u64 = 86_400;

struct Options {
    senders: usize,
    txs: usize,
    rate: f64,
    allow_debug: bool,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut o = Options {
        senders: 30,
        txs: 10,
        rate: 10.0,
        allow_debug: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| -> Result<String> {
            it.next().cloned().ok_or(format!("{name} needs a value"))
        };
        match a.as_str() {
            "--senders" => o.senders = val("--senders")?.parse().map_err(|e| format!("{e}"))?,
            "--txs" => o.txs = val("--txs")?.parse().map_err(|e| format!("{e}"))?,
            "--rate" => o.rate = val("--rate")?.parse().map_err(|e| format!("{e}"))?,
            "--allow-debug" => o.allow_debug = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if o.senders == 0 || o.txs == 0 || !(o.rate.is_finite() && o.rate > 0.0) {
        return Err("--senders and --txs must be at least 1 and --rate positive".into());
    }
    Ok(o)
}

/// Nearest-rank percentile of an ascending slice; `None` when empty.
fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (p / 100.0 * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted.get(rank - 1).copied()
}

#[derive(Debug, PartialEq)]
struct Summary {
    n: usize,
    p50: f64,
    p90: f64,
    p99: f64,
    max: f64,
}

fn summarize(samples: &[f64]) -> Option<Summary> {
    let mut s = samples.to_vec();
    s.sort_by(f64::total_cmp);
    Some(Summary {
        n: s.len(),
        p50: percentile(&s, 50.0)?,
        p90: percentile(&s, 90.0)?,
        p99: percentile(&s, 99.0)?,
        max: *s.last()?,
    })
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

fn date_stamp(unix_secs: u64) -> String {
    let (y, m, d) = civil_from_days(unix_secs / SECS_PER_DAY);
    format!("{y:04}-{m:02}-{d:02}")
}

/// What a report needs besides the samples.
struct Meta {
    command: String,
    commit: String,
    os: String,
    hardware: String,
    senders: usize,
    txs: usize,
    rate: f64,
    submitted: usize,
    accepted: usize,
    submit_secs: f64,
}

fn row(name: &str, s: Option<&Summary>) -> String {
    s.map_or_else(
        || format!("| {name} | 0 | MISSING | MISSING | MISSING | MISSING |\n"),
        |s| {
            format!(
                "| {name} | {} | {:.1} | {:.1} | {:.1} | {:.1} |\n",
                s.n, s.p50, s.p90, s.p99, s.max
            )
        },
    )
}

fn render(m: &Meta, included: &[f64], finalized: &[f64]) -> String {
    let achieved = m.accepted as f64 / m.submit_secs;
    let mut out = format!(
        "# bench-finality (localnet, {VALIDATORS} validators on one machine)\n\n\
         - commit: {}\n- os: {}\n- hardware: {}\n- command: {}\n\
         - mix: single-output transfers of 1 base unit, {} senders x {} nonces to one recipient, hybrid ML-DSA-65 + SLH-DSA\n\
         - offered rate: {:.1} tx/s open loop; achieved {achieved:.1} accepted/s over {:.2}s\n\
         - posted: {}; accepted by a mempool: {}; included: {}; finalized: {}\n\n\
         Latency in milliseconds, nearest-rank percentiles, from the moment the post is sent:\n\n\
         | clock | n | p50 | p90 | p99 | max |\n|---|---|---|---|---|---|\n",
        m.commit,
        m.os,
        m.hardware,
        m.command,
        m.senders,
        m.txs,
        m.rate,
        m.submit_secs,
        m.submitted,
        m.accepted,
        included.len(),
        finalized.len(),
    );
    out.push_str(&row("submit to included", summarize(included).as_ref()));
    out.push_str(&row("submit to finalized", summarize(finalized).as_ref()));
    out.push_str(
        "\n- included: node 3 serves a block carrying the txid (polled every 10 ms, so +/- 10 ms).\n\
         - finalized: node 3's `get_checkpoint` is a quorum-attested block (2f+1, ADR-038) at or above the including block.\n\n\
         Single machine, localhost links, other processes were running: no network latency is in these figures, \
         and a real network will be slower. Signing excluded. Transactions not finalized are counted above, not hidden.\n\
         Not measured: WAN, separate machines, contract calls, mixed loads, rates other than the one stated.\n",
    );
    out
}

/// First free `finality-<date>[-n].md` in `dir`; never overwrites a report.
fn report_path(dir: &Path, date: &str) -> PathBuf {
    let mut path = dir.join(format!("finality-{date}.md"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("finality-{date}-{n}.md"));
        n += 1;
    }
    path
}

fn write_report(dir: &Path, date: &str, text: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = report_path(dir, date);
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn call(node: u16, method: &str, params: &Value) -> Result<Value> {
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let (_, reply) = devnet::post(&localnet::rpc_addr(node), "/", &request)?;
    Ok(reply.get("result").cloned().unwrap_or(Value::Null))
}

struct Seen {
    height: u64,
    included: Instant,
    finalized: Option<Instant>,
}

/// Polls the observer until every accepted tx is finalized or progress stalls.
fn watch(from_height: u64, submitting: &AtomicBool, accepted: &AtomicU64) -> HashMap<String, Seen> {
    let mut seen: HashMap<String, Seen> = HashMap::new();
    let mut next = from_height + 1;
    let mut moved = Instant::now();
    loop {
        let tip = call(OBSERVER, "get_chain_info", &json!([]))
            .ok()
            .and_then(|v| v["height"].as_u64())
            .unwrap_or(0);
        while next <= tip {
            let Ok(block) = call(OBSERVER, "get_block_by_height", &json!([next])) else {
                break;
            };
            let at = Instant::now();
            for t in block["transactions"].as_array().into_iter().flatten() {
                if let Some(id) = t["txid"].as_str() {
                    seen.entry(id.to_owned()).or_insert(Seen {
                        height: next,
                        included: at,
                        finalized: None,
                    });
                }
            }
            next += 1;
        }
        let checkpoint = call(OBSERVER, "get_checkpoint", &json!([]))
            .ok()
            .and_then(|v| v["height"].as_u64())
            .unwrap_or(0);
        let at = Instant::now();
        for s in seen
            .values_mut()
            .filter(|s| s.finalized.is_none() && s.height <= checkpoint)
        {
            s.finalized = Some(at);
            moved = at;
        }
        let done = seen.values().filter(|s| s.finalized.is_some()).count() as u64;
        let idle = !submitting.load(Ordering::SeqCst);
        if idle && (done >= accepted.load(Ordering::SeqCst) || moved.elapsed() > STALL) {
            return seen;
        }
        if submitting.load(Ordering::SeqCst) {
            moved = Instant::now();
        }
        std::thread::sleep(POLL);
    }
}

/// Posts tx `k` at `start + k / rate` to node `k % 4`; returns (txid, post instant).
fn post_scheduled(
    raw: &str,
    k: usize,
    node: u16,
    start: Instant,
    rate: f64,
) -> Option<(String, Instant)> {
    let due = start + Duration::from_secs_f64(k as f64 / rate);
    if let Some(wait) = due.checked_duration_since(Instant::now()) {
        std::thread::sleep(wait);
    }
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": "send_raw_transaction", "params": [raw]});
    let sent = Instant::now();
    let (_, reply) = devnet::post(&localnet::rpc_addr(node), "/", &request).ok()?;
    let txid = reply.pointer("/result/txid")?.as_str()?.to_owned();
    Some((txid, sent))
}

fn run_load(
    o: &Options,
    signed: &[Vec<String>],
    from_height: u64,
) -> (usize, Vec<(String, Instant)>, HashMap<String, Seen>, f64) {
    let submitting = AtomicBool::new(true);
    let accepted = AtomicU64::new(0);
    let posts: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());
    let total = o.senders * o.txs;
    let mut secs = 0.0;
    let seen = std::thread::scope(|scope| {
        let watcher = scope.spawn(|| watch(from_height, &submitting, &accepted));
        let start = Instant::now();
        // Index k = nonce * senders + sender: each sender's nonces stay in order.
        let _ = parallel(total, SUBMIT_THREADS, &|k| {
            let raw = &signed[k % o.senders][k / o.senders];
            let node = u16::try_from(k % usize::from(VALIDATORS)).unwrap_or(0);
            if let Some(p) = post_scheduled(raw, k, node, start, o.rate) {
                accepted.fetch_add(1, Ordering::SeqCst);
                posts
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(p);
            }
            Ok(())
        });
        secs = start.elapsed().as_secs_f64();
        submitting.store(false, Ordering::SeqCst);
        watcher.join().unwrap_or_default()
    });
    let posts = posts
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    (total, posts, seen, secs)
}

/// Per-clock latency samples in ms for every accepted tx the observer saw.
fn latencies(posts: &[(String, Instant)], seen: &HashMap<String, Seen>) -> (Vec<f64>, Vec<f64>) {
    let ms = |later: Instant, sent: Instant| {
        later.saturating_duration_since(sent).as_secs_f64() * 1_000.0
    };
    let mut inc = Vec::new();
    let mut fin = Vec::new();
    for (id, sent) in posts {
        if let Some(s) = seen.get(id) {
            inc.push(ms(s.included, *sent));
            fin.extend(s.finalized.map(|f| ms(f, *sent)));
        }
    }
    (inc, fin)
}

fn hardware() -> String {
    let cores = std::thread::available_parallelism().map_or(0, std::num::NonZero::get);
    let cpu = std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "MISSING".into());
    format!("{cpu}; {cores} logical cores; RAM MISSING")
}

/// Entry point.
///
/// # Errors
///
/// A debug build, busy ports, a node that does not come up, or no accepted
/// transaction.
pub fn bench_finality(args: &[String]) -> Result<()> {
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
    let senders = make_senders(&root.join("bench-finality-keys"), o.senders)?;
    let work = root.join("bench-finality");
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
    println!("bench-finality: signing {} transfers", o.senders * o.txs);
    let signed = presign(&senders, o.txs)?;
    let from = localnet::tip(OBSERVER, &genesis_address)?.0;
    let (submitted, posts, seen, secs) = run_load(&o, &signed, from);
    let (inc, fin) = latencies(&posts, &seen);
    let meta = Meta {
        command: format!("cargo xtask bench-finality {}", args.join(" "))
            .trim_end()
            .to_owned(),
        commit: command_output("git", &["rev-parse", "--short", "HEAD"]),
        os: format!(
            "{} / {}",
            std::env::consts::OS,
            command_output("cmd", &["/C", "ver"])
        ),
        hardware: hardware(),
        senders: o.senders,
        txs: o.txs,
        rate: o.rate,
        submitted,
        accepted: posts.len(),
        submit_secs: secs,
    };
    let text = render(&meta, &inc, &fin);
    println!("{text}");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let path = write_report(
        &devnet::root().join("reports").join("bench"),
        &date_stamp(stamp),
        &text,
    )?;
    println!("bench-finality: wrote {}", path.display());
    if posts.is_empty() || fin.len() < posts.len() {
        return Err(format!(
            "{} of {} accepted transfers finalized",
            fin.len(),
            posts.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn nearest_rank_percentiles() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        let s = summarize(&v).unwrap();
        assert_eq!(
            (s.p50, s.p90, s.p99, s.max, s.n),
            (50.0, 90.0, 99.0, 100.0, 100)
        );
    }

    #[test]
    fn percentiles_of_one_and_unsorted_input() {
        let s = summarize(&[7.0]).unwrap();
        assert_eq!((s.p50, s.p99, s.max), (7.0, 7.0, 7.0));
        let s = summarize(&[30.0, 10.0, 20.0, 40.0]).unwrap();
        assert_eq!((s.p50, s.p90, s.max), (20.0, 40.0, 40.0));
        assert_eq!(summarize(&[]), None);
    }

    #[test]
    fn dates_are_civil() {
        assert_eq!(date_stamp(0), "1970-01-01");
        assert_eq!(date_stamp(1_078_012_800), "2004-02-29");
        assert_eq!(date_stamp(1_783_641_600), "2026-07-10");
    }

    fn meta() -> Meta {
        Meta {
            command: "cargo xtask bench-finality --rate 5".into(),
            commit: "abc1234".into(),
            os: "windows".into(),
            hardware: "cpu".into(),
            senders: 2,
            txs: 3,
            rate: 5.0,
            submitted: 6,
            accepted: 6,
            submit_secs: 1.2,
        }
    }

    #[test]
    fn report_states_provenance_and_caveat() {
        let t = render(&meta(), &[10.0, 20.0], &[100.0, 200.0]);
        for needle in [
            "commit: abc1234",
            "command: cargo xtask bench-finality --rate 5",
            "| submit to included | 2 | 10.0 | 20.0 | 20.0 | 20.0 |",
            "| submit to finalized | 2 | 100.0 | 200.0 | 200.0 | 200.0 |",
            "Single machine, localhost",
        ] {
            assert!(t.contains(needle), "missing {needle}");
        }
    }

    #[test]
    fn empty_samples_print_missing_not_zero() {
        assert!(render(&meta(), &[], &[]).contains("| submit to finalized | 0 | MISSING"));
    }

    #[test]
    fn writer_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("bf-test-{}", std::process::id()));
        let a = write_report(&dir, "2026-01-02", "one").unwrap();
        let b = write_report(&dir, "2026-01-02", "two").unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "one");
        assert!(b.ends_with("finality-2026-01-02-2.md"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn options_reject_nonsense() {
        let a = |s: &[&str]| parse(&s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>());
        assert!(a(&["--rate", "0"]).is_err());
        assert!(a(&["--senders", "0"]).is_err());
        assert!(a(&["--bogus"]).is_err());
        assert!((a(&["--rate", "2.5"]).unwrap().rate - 2.5).abs() < f64::EPSILON);
    }
}
