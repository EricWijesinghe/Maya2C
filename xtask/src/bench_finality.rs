//! `cargo xtask bench-finality [--txs N] [--rate R] [--allow-debug]` —
//! submit-to-finalized latency per transfer on the four-validator localnet.
//!
//! Method (ADR-059, Standing Order 2): `N` single-transfer transactions, one
//! funded sender each, are signed first (client work, not timed). They are
//! then submitted at a fixed rate `R` to nodes 0..2, round-robin. A transfer
//! is finalized when a block carrying it is served as canonical by node 3,
//! which never received it from a client. A node only applies a block once
//! DAG-BFT commits its anchor (ADR-027), so "canonical on node 3" is the
//! finality bar here. The node exposes no per-transaction finality flag, so
//! this tool polls `get_block_by_height` on node 3 (every 20 ms). Latency is
//! `observed - submitted`, where `submitted` is taken just before the
//! `send_raw_transaction` call. Resolution is the poll interval plus one RPC
//! round trip. Both clocks are this process's, on the same machine.
//!
//! Not measured: mixed loads, contract calls, WAN latency, validators on
//! separate machines, sustained load. Every report says so.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::bench_tps;
use crate::devnet::{self, Procs, Result};
use crate::localnet::{self, VALIDATORS};

/// Clear of `bench-tps` (3000) and of the live testnet ports.
const PORT_OFFSET: u16 = 3_100;
const POLL: Duration = Duration::from_millis(20);
const DEFAULT_TXS: usize = 200;
const DEFAULT_RATE: f64 = 20.0;

struct Options {
    txs: usize,
    rate: f64,
    allow_debug: bool,
}

fn parse(args: &[String]) -> Result<Options> {
    let mut o = Options {
        txs: DEFAULT_TXS,
        rate: DEFAULT_RATE,
        allow_debug: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |name: &str| -> Result<String> {
            it.next().cloned().ok_or(format!("{name} needs a value"))
        };
        match a.as_str() {
            "--txs" => o.txs = value("--txs")?.parse().map_err(|e| format!("--txs: {e}"))?,
            "--rate" => {
                o.rate = value("--rate")?
                    .parse()
                    .map_err(|e| format!("--rate: {e}"))?
            }
            "--allow-debug" => o.allow_debug = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if o.txs == 0 {
        return Err("--txs must be at least 1".into());
    }
    if !(o.rate.is_finite() && o.rate > 0.0) {
        return Err("--rate must be a positive number of transfers per second".into());
    }
    Ok(o)
}

/// What happened to one transfer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Outcome {
    /// The node refused the submission, so it is not counted in the latency.
    Refused,
    /// Accepted but no canonical block on node 3 before the stall limit.
    NeverFinalized,
    /// Submit-to-finalized latency in milliseconds.
    Finalized(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Summary {
    pub attempted: usize,
    pub refused: usize,
    pub finalized: usize,
    pub never_finalized: usize,
    pub p50_ms: Option<f64>,
    pub p90_ms: Option<f64>,
    pub p99_ms: Option<f64>,
    pub max_ms: Option<f64>,
}

/// Nearest-rank percentile of an ascending slice (`None` when empty).
pub(crate) fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    // 1-based nearest rank: ceil(p/100 * n), clamped to the sample.
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted.get(rank.clamp(1, sorted.len()) - 1).copied()
}

/// Percentiles are over finalized transfers only; the rest is counted apart.
pub(crate) fn summarize(outcomes: &[Outcome]) -> Summary {
    let count = |want: Outcome| outcomes.iter().filter(|o| **o == want).count();
    let mut ms: Vec<f64> = outcomes
        .iter()
        .filter_map(|o| match o {
            Outcome::Finalized(x) => Some(*x),
            _ => None,
        })
        .collect();
    ms.sort_by(f64::total_cmp);
    Summary {
        attempted: outcomes.len(),
        refused: count(Outcome::Refused),
        finalized: ms.len(),
        never_finalized: count(Outcome::NeverFinalized),
        p50_ms: percentile(&ms, 50.0),
        p90_ms: percentile(&ms, 90.0),
        p99_ms: percentile(&ms, 99.0),
        max_ms: ms.last().copied(),
    }
}

/// One submission: the txid the node returned, or why it refused.
struct Attempt {
    txid: std::result::Result<String, String>,
    sent: Instant,
}

/// Submits `raws[i]` at `start + i / rate` to node `i % (VALIDATORS - 1)`.
fn submit_at_rate(raws: &[String], rate: f64) -> Vec<Attempt> {
    let start = Instant::now();
    let mut out = Vec::with_capacity(raws.len());
    for (i, raw) in raws.iter().enumerate() {
        let due = start + Duration::from_secs_f64(i as f64 / rate);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        let node = u16::try_from(i % usize::from(VALIDATORS - 1)).unwrap_or(0);
        let request = json!({
            "jsonrpc": "2.0", "id": 1, "method": "send_raw_transaction", "params": [raw]
        });
        let sent = Instant::now();
        let txid = match devnet::post(&localnet::rpc_addr(node), "/", &request) {
            Ok((_, v)) => match (
                v.get("error"),
                v.pointer("/result/txid").and_then(Value::as_str),
            ) {
                (None, Some(id)) => Ok(id.to_owned()),
                (Some(error), _) => Err(error.to_string()),
                (None, None) => Err("reply carried no txid".to_owned()),
            },
            Err(e) => Err(e),
        };
        out.push(Attempt { txid, sent });
    }
    out
}

/// Transaction ids in canonical block `height` on `node`, or `None` when the
/// node has no such block (yet).
fn block_txids(node: u16, height: u64) -> Option<Vec<String>> {
    let request =
        json!({"jsonrpc": "2.0", "id": 1, "method": "get_block_by_height", "params": [height]});
    let (status, reply) = devnet::post(&localnet::rpc_addr(node), "/", &request).ok()?;
    if status >= 400 {
        return None;
    }
    let txs = reply.pointer("/result/transactions")?.as_array()?;
    Some(
        txs.iter()
            .filter_map(|t| t["txid"].as_str().map(str::to_owned))
            .collect(),
    )
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Follows node 3's canonical chain from genesis and stamps every transaction
/// it first sees. Stops once every id in `wanted` is seen, or after `stall`
/// with no new block. `wanted` is set after submission ends.
fn observe(wanted: &Mutex<Option<Vec<String>>>, stall: Duration) -> BTreeMap<String, Instant> {
    let node = VALIDATORS - 1;
    let mut seen = BTreeMap::new();
    let mut height = 0u64;
    let mut moved = Instant::now();
    loop {
        let done = lock(wanted)
            .as_ref()
            .is_some_and(|ids| ids.iter().all(|id| seen.contains_key(id)));
        if done || moved.elapsed() > stall {
            return seen;
        }
        match block_txids(node, height) {
            Some(ids) => {
                let at = Instant::now();
                for id in ids {
                    seen.entry(id).or_insert(at);
                }
                height += 1;
                moved = Instant::now();
            }
            None => std::thread::sleep(POLL),
        }
    }
}

fn outcomes(attempts: &[Attempt], observed: &BTreeMap<String, Instant>) -> Vec<Outcome> {
    attempts
        .iter()
        .map(|a| match &a.txid {
            Err(_) => Outcome::Refused,
            Ok(id) => observed.get(id).map_or(Outcome::NeverFinalized, |at| {
                Outcome::Finalized(at.duration_since(a.sent).as_secs_f64() * 1000.0)
            }),
        })
        .collect()
}

fn cell(x: Option<f64>) -> String {
    x.map_or_else(|| "MISSING".to_owned(), |v| format!("{v:.2}"))
}

fn report(o: &Options, s: &Summary, first_refusal: Option<&str>, stamp: &str) -> (Value, String) {
    let json = json!({
        "commit": bench_tps::command_output("git", &["rev-parse", "--short", "HEAD"]),
        "os": std::env::consts::OS,
        "cpu": std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "MISSING".into()),
        "logical_cores": std::thread::available_parallelism().map_or(0, std::num::NonZero::get),
        "validators": VALIDATORS,
        "machine": "one machine, localhost, shared with other processes",
        "command": format!("cargo xtask bench-finality --txs {} --rate {}", o.txs, o.rate),
        "mix": "single-output transfers of 1 base unit, one funded sender each, to one recipient; hybrid ML-DSA-65 + SLH-DSA",
        "submit_rate_per_s": o.rate,
        "poll_interval_ms": POLL.as_millis(),
        "stamp": stamp,
        "attempted": s.attempted,
        "refused": s.refused,
        "finalized": s.finalized,
        "never_finalized": s.never_finalized,
        "latency_ms": { "p50": s.p50_ms, "p90": s.p90_ms, "p99": s.p99_ms, "max": s.max_ms },
    });
    let md = format!(
        "# bench-finality (localnet, {VALIDATORS} validators, ONE machine, localhost)\n\n\
         - commit: {}\n- os: {} / cpu: {} / logical cores: {}\n\
         - binaries: {}\n- command: {}\n\
         - mix: {}\n\
         - submit schedule: {:.1}/s round-robin to nodes 0..2; observed on node 3 (never sent the transfer)\n\
         - observation: polling canonical blocks every {} ms; latency includes the poll and one RPC round trip\n\n\
         | attempted | refused | finalized | never finalized |\n|---|---|---|---|\n\
         | {} | {} | {} | {} |\n\n\
         | p50 ms | p90 ms | p99 ms | max ms |\n|---|---|---|---|\n\
         | {} | {} | {} | {} |\n\n\
         {}\n\n\
         Localhost, one machine, shared with other processes. Not a network figure. \
         Not measured: mixed loads, contract calls, WAN latency, separate machines, sustained load.\n",
        json["commit"].as_str().unwrap_or("MISSING"),
        std::env::consts::OS,
        json["cpu"].as_str().unwrap_or("MISSING"),
        json["logical_cores"],
        devnet::bin("maya2c-node").display(),
        json["command"].as_str().unwrap_or(""),
        json["mix"].as_str().unwrap_or(""),
        o.rate,
        POLL.as_millis(),
        s.attempted,
        s.refused,
        s.finalized,
        s.never_finalized,
        cell(s.p50_ms),
        cell(s.p90_ms),
        cell(s.p99_ms),
        cell(s.max_ms),
        first_refusal.map_or_else(String::new, |e| format!("First refusal: `{e}`")),
    );
    (json, md)
}

/// Entry point.
///
/// # Errors
///
/// A debug build, busy ports, a node that does not come up, or no transfer
/// finalized at all (then there is no latency figure to report).
pub fn bench_finality(args: &[String]) -> Result<()> {
    let o = parse(args)?;
    bench_tps::refuse_debug(o.allow_debug)?;
    devnet::build(&["maya2c-node", "l1-wallet"])?;
    localnet::use_port_offset(PORT_OFFSET);
    localnet::ports_free()?;
    let root = devnet::root().join("target");
    let senders = bench_tps::make_senders(&root.join("bench-finality-keys"), o.txs)?;
    let addresses: Vec<String> = senders.iter().map(|(_, a)| a.clone()).collect();
    let work = root.join("bench-finality");
    let genesis = bench_tps::setup_funded(&work, &addresses)?;
    let mut procs = Procs::default();
    for i in 0..VALIDATORS {
        procs.push(localnet::start(&work, i)?);
    }
    let all: Vec<u16> = (0..VALIDATORS).collect();
    localnet::wait_until("four nodes on one chain", || {
        localnet::agree(&all, &genesis, 3)
    })?;
    println!("bench-finality: signing {} transfers", o.txs);
    let raws: Vec<String> = bench_tps::presign(&senders, 1)?
        .into_iter()
        .flatten()
        .collect();
    let wanted = Mutex::new(None);
    let stall = Duration::from_secs(bench_tps::STALL_SECS);
    let (attempts, observed) = std::thread::scope(|scope| -> Result<_> {
        let watcher = scope.spawn(|| observe(&wanted, stall));
        let attempts = submit_at_rate(&raws, o.rate);
        let ids = attempts
            .iter()
            .filter_map(|a| a.txid.as_ref().ok().cloned())
            .collect();
        *lock(&wanted) = Some(ids);
        let observed = watcher
            .join()
            .map_err(|_| "observer thread panicked".to_owned())?;
        Ok((attempts, observed))
    })?;
    drop(procs);
    let first_refusal = attempts.iter().find_map(|a| a.txid.as_ref().err().cloned());
    let summary = summarize(&outcomes(&attempts, &observed));
    let stamp = chrono::Local::now().format("%Y-%m-%d-%H%M%S").to_string();
    let (json, text) = report(&o, &summary, first_refusal.as_deref(), &stamp);
    println!("{text}");
    let dir = devnet::root().join("reports").join("bench");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let body = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("finality-{stamp}.json")), body).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("finality-{stamp}.md")), &text).map_err(|e| e.to_string())?;
    if summary.finalized == 0 {
        return Err("no transfer finalized: there is no latency figure to report".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fin(xs: &[f64]) -> Vec<Outcome> {
        xs.iter().map(|x| Outcome::Finalized(*x)).collect()
    }

    #[test]
    fn nearest_rank_matches_the_definition() {
        let s: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&s, 50.0), Some(50.0));
        assert_eq!(percentile(&s, 90.0), Some(90.0));
        assert_eq!(percentile(&s, 99.0), Some(99.0));
        assert_eq!(percentile(&s, 100.0), Some(100.0));
        assert_eq!(percentile(&s, 0.0), Some(1.0));
        let three = [10.0, 20.0, 30.0];
        assert_eq!(percentile(&three, 50.0), Some(20.0));
        assert_eq!(percentile(&three, 90.0), Some(30.0));
    }

    #[test]
    fn empty_sample_has_no_percentile() {
        assert_eq!(percentile(&[], 50.0), None);
        let s = summarize(&[]);
        assert_eq!((s.attempted, s.finalized, s.never_finalized), (0, 0, 0));
        assert_eq!(
            (s.p50_ms, s.p90_ms, s.p99_ms, s.max_ms),
            (None, None, None, None)
        );
    }

    #[test]
    fn never_finalized_is_counted_and_excluded_from_latency() {
        let s = summarize(&[Outcome::NeverFinalized, Outcome::NeverFinalized]);
        assert_eq!((s.attempted, s.finalized, s.never_finalized), (2, 0, 2));
        assert_eq!((s.p50_ms, s.max_ms), (None, None));
    }

    #[test]
    fn refusals_and_unsorted_latencies_aggregate_correctly() {
        let mut v = vec![Outcome::Refused, Outcome::NeverFinalized];
        v.extend(fin(&[5.0, 1.0, 3.0]));
        let s = summarize(&v);
        assert_eq!(
            (s.attempted, s.refused, s.finalized, s.never_finalized),
            (5, 1, 3, 1)
        );
        assert_eq!(s.p50_ms, Some(3.0));
        assert_eq!(s.p99_ms, Some(5.0));
        assert_eq!(s.max_ms, Some(5.0));
    }
}
