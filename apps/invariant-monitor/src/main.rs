//! `maya2c-invariant-monitor`: polls public endpoints and reports every
//! invariant violation a third party can see. Read-only.
//!
//! ```text
//! maya2c-invariant-monitor --node gw=https://rpc.example/rpc --node mine=http://127.0.0.1:8545 [--once]
//! ```
//!
//! Findings go to stdout. With `MAYA2C_MONITOR_NTFY_TOPIC` set (and no
//! `--dry-run`) they are also pushed to that ntfy topic. `--once` polls one
//! time and exits 1 if anything was found, for cron or CI. A single poll has
//! no history, so the regress checks and the halt timer cannot fire with it.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use maya_invariant_monitor::checks::PublicChecks;
use maya_invariant_monitor::public;
use maya_sentinel::config::{self, Command};
use maya_sentinel::detect::Detector;
use maya_sentinel::incident::{Event, Incidents};
use maya_sentinel::{notify, obs, rpc};

const TOPIC_ENV: &str = "MAYA2C_MONITOR_NTFY_TOPIC";
/// `get_supply` scans every account: once a minute at the default poll.
const SUPPLY_EVERY: u64 = 12;
/// A condition must be gone this long before its incident resolves.
const CLEAR_MS: u64 = 30_000;
const USAGE: &str = "usage: maya2c-invariant-monitor --node NAME=URL [--node NAME=URL]... \
[--once] [--poll-secs 5] [--halt-secs 30] [--unreachable-secs 45] [--dry-run]\n\
env MAYA2C_MONITOR_NTFY_TOPIC: optional ntfy topic; stdout always";

/// Exit code of `--once` when a violation was found.
const FOUND: u8 = 1;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("maya2c-invariant-monitor: {e}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

async fn run() -> Result<u8, String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let once = args.iter().any(|a| a == "--once");
    args.retain(|a| a != "--once");
    let cfg = match config::parse_args(args)? {
        Command::Help => {
            println!("{USAGE}");
            return Ok(0);
        }
        Command::Run(cfg) => cfg,
    };
    let topic = match std::env::var(TOPIC_ENV) {
        Ok(raw) if !cfg.dry_run => Some(notify::valid_topic(&raw)?.to_owned()),
        _ => None,
    };
    let nodes = cfg
        .nodes
        .iter()
        .map(|(name, url)| rpc::client(url).map(|c| (name.clone(), c)))
        .collect::<Result<Vec<_>, _>>()?;
    println!(
        "START sources={} (get_peers and get_mempool_info are not public: peer and stranding checks idle)",
        nodes.len()
    );

    let http = reqwest::Client::new();
    let start = Instant::now();
    let mut detector = Detector::new(cfg.thresholds);
    let mut checks = PublicChecks::new();
    let mut incidents = Incidents::new(CLEAR_MS);
    let mut rate = notify::RateLimit::new(config::DEFAULT_RATE_MAX, config::DEFAULT_RATE_WINDOW_MS);
    let mut poll: u64 = 0;
    loop {
        let now = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let samples = obs::gather(&nodes, once || poll.is_multiple_of(SUPPLY_EVERY)).await;
        let publics = public::gather_public(&nodes).await;
        let mut findings = detector.evaluate(now, &samples);
        findings.extend(checks.evaluate(&samples, &publics));
        if once {
            for f in &findings {
                println!("VIOLATION {} :: {}", f.title, f.body);
            }
            return Ok(if findings.is_empty() { 0 } else { FOUND });
        }
        for event in incidents.update(now, findings) {
            let msg = notify::render(&event);
            let kind = if matches!(event, Event::Raised(_)) {
                "RAISED"
            } else {
                "RESOLVED"
            };
            println!("{kind} P{} {} :: {}", msg.priority, msg.title, msg.body);
            // Stdout always; pushes are rate-limited like the sentinel's.
            if let Some(t) = &topic
                && rate.allow(now)
                && let Err(e) = notify::send(&http, notify::NTFY_BASE, t, &msg).await
            {
                eprintln!("ntfy send failed: {e}");
            }
        }
        poll += 1;
        tokio::time::sleep(Duration::from_millis(cfg.poll_ms)).await;
    }
}
