//! `pool-load` — measures the pool's share path with simulated miners on localhost.
//!
//! ```text
//! pool-load run --miners 2000 --per-miner-rate 0.2 --duration 30 --invalid-ratio 0.05
//! ```
//!
//! ## Two processes, so the memory numbers mean something
//!
//! `run` (the orchestrator and the simulated miners) spawns `serve` as a child
//! process: the pool itself, with the real RocksDB ledger, the real validator
//! pool, and the real Stratum listener. Server memory is read from the child's
//! own working set, so it excludes the miners' sockets and buffers.
//!
//! ## What is timed, and what is not
//!
//! Solving shares is done before the timed phase, on the miners' side, and is
//! reported as its own number. The timed phase only sends pre-solved shares
//! over the encrypted transport, so the latencies and the accepted-share rate
//! are the pool's cost. Valid shares are real proof-of-work solutions checked
//! by the pool with the chain's `DagConfig::MAINNET`; invalid shares are real
//! nonces whose digest misses the channel target, and the pool hashes them too.
//!
//! ## Difficulty is pinned
//!
//! `min_target_bits == max_target_bits`, so vardiff cannot move a channel's
//! target during the run. Every share costs the same pool work whatever its
//! target, so the pinned value changes solving cost on the miner side only.

mod cost;
mod miner;
mod run;
mod serve;
mod working_set;

use std::error::Error;
use std::path::PathBuf;

/// Share difficulty for simulated channels, in leading zero bits.
///
/// Pinned (min == max) so vardiff cannot retarget mid-run. Three bits is one
/// solution in eight hashes, which keeps miner-side solving cheap.
pub const SHARE_BITS: u32 = 3;

/// Network difficulty the candidate advertises, in leading zero bits.
///
/// Far above the share bits so no simulated share also solves the chain, and
/// the block path (`FoundBlockShare`) stays idle.
pub const NETWORK_BITS: u32 = 60;

fn usage() -> &'static str {
    "pool-load run   [--miners N] [--per-miner-rate R] [--duration S] [--invalid-ratio F]\n\
     \x20               [--validators N] [--connect-batch N] [--job-push] [--ledger-dir DIR]\n\
     pool-load serve [--validators N] --ledger-dir DIR   (spawned by `run`; not for direct use)"
}

/// Parses `--flag value` pairs into a lookup, rejecting anything unknown.
fn parse_flags(
    argv: &[String],
    known: &[&str],
    switches: &[&str],
) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let mut out = Vec::new();
    let mut iter = argv.iter();
    while let Some(flag) = iter.next() {
        if !known.contains(&flag.as_str()) && !switches.contains(&flag.as_str()) {
            return Err(format!("unknown flag {flag}\n{}", usage()).into());
        }
        if switches.contains(&flag.as_str()) {
            out.push((flag.clone(), String::new()));
            continue;
        }
        let value = iter
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?;
        out.push((flag.clone(), value.clone()));
    }
    Ok(out)
}

fn get<'a>(flags: &'a [(String, String)], name: &str) -> Option<&'a str> {
    flags
        .iter()
        .rev()
        .find(|(flag, _)| flag == name)
        .map(|(_, value)| value.as_str())
}

fn parse_or<T: std::str::FromStr>(
    flags: &[(String, String)],
    name: &str,
    default: T,
) -> Result<T, Box<dyn Error>>
where
    T::Err: Error + 'static,
{
    match get(flags, name) {
        Some(value) => Ok(value.parse()?),
        None => Ok(default),
    }
}

fn default_ledger_dir() -> PathBuf {
    std::env::temp_dir().join(format!("pool-load-{}", std::process::id()))
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = argv.split_first() else {
        return Err(usage().into());
    };

    match command.as_str() {
        "serve" => {
            let flags = parse_flags(rest, &["--validators", "--ledger-dir"], &[])?;
            let validators = parse_or(&flags, "--validators", maya_pool_service::validator::DEFAULT_WORKERS)?;
            let dir = get(&flags, "--ledger-dir")
                .map(PathBuf::from)
                .ok_or("serve needs --ledger-dir")?;
            serve::run(validators, &dir).await
        }
        "hash-cost" => cost::run(),
        "run" => {
            let flags = parse_flags(
                rest,
                &[
                    "--miners",
                    "--per-miner-rate",
                    "--duration",
                    "--invalid-ratio",
                    "--validators",
                    "--connect-batch",
                    "--ledger-dir",
                ],
                &["--job-push"],
            )?;
            let settings = run::Settings {
                miners: parse_or(&flags, "--miners", 1_000)?,
                per_miner_rate: parse_or(&flags, "--per-miner-rate", 0.2)?,
                duration_secs: parse_or(&flags, "--duration", 30)?,
                invalid_ratio: parse_or(&flags, "--invalid-ratio", 0.05)?,
                validators: parse_or(&flags, "--validators", maya_pool_service::validator::DEFAULT_WORKERS)?,
                connect_batch: parse_or(&flags, "--connect-batch", 256)?,
                job_push: flags.iter().any(|(flag, _)| flag == "--job-push"),
                ledger_dir: get(&flags, "--ledger-dir")
                    .map_or_else(default_ledger_dir, PathBuf::from),
            };
            run::execute(settings).await
        }
        _ => Err(usage().into()),
    }
}
