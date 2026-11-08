//! `maya2c-sentinel`: polls the nodes, pages on incidents, never touches the chain.
//!
//! ```text
//! MAYA2C_SENTINEL_NTFY_TOPIC=<private-topic> maya2c-sentinel \
//!     --node seed=http://127.0.0.1:8545 --node v2=http://127.0.0.1:8546
//! ```
//!
//! `--dry-run` prints what would be pushed and needs no topic.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use maya_sentinel::config::{self, Command, Config, USAGE};
use maya_sentinel::detect::Detector;
use maya_sentinel::incident::{Event, Incidents};
use maya_sentinel::notify::{self, RateLimit};
use maya_sentinel::obs::{self, Sample};
use maya_sentinel::rpc;

/// Env var holding the ntfy topic. Never a flag, never a default.
const TOPIC_ENV: &str = "MAYA2C_SENTINEL_NTFY_TOPIC";
/// Polls between supply reads. `get_supply` scans every account, so it runs
/// once a minute at the default 5 s poll rather than every poll.
const SUPPLY_EVERY: u64 = 12;
/// A condition must be gone this long before its incident resolves.
const CLEAR_MS: u64 = 30_000;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("maya2c-sentinel: {e}");
            ExitCode::from(2)
        }
    }
}

async fn run() -> Result<(), String> {
    let cfg = match config::parse_args(std::env::args().skip(1))? {
        Command::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        Command::Run(cfg) => cfg,
    };
    let topic = if cfg.dry_run {
        None
    } else {
        Some(read_topic()?)
    };
    let nodes = cfg
        .nodes
        .iter()
        .map(|(name, url)| rpc::client(url).map(|c| (name.clone(), c)))
        .collect::<Result<Vec<_>, _>>()?;

    let http = reqwest::Client::new();
    let start = Instant::now();
    let mut detector = Detector::new(cfg.thresholds);
    let mut incidents = Incidents::new(CLEAR_MS);
    let mut rate = RateLimit::new(config::DEFAULT_RATE_MAX, config::DEFAULT_RATE_WINDOW_MS);
    let mut poll: u64 = 0;
    loop {
        let now = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let samples = obs::gather(&nodes, poll.is_multiple_of(SUPPLY_EVERY)).await;
        if poll == 0 {
            note_startup(&cfg, &samples);
        }
        for event in incidents.update(now, detector.evaluate(now, &samples)) {
            deliver(&cfg, &http, topic.as_deref(), &mut rate, now, &event).await;
        }
        poll += 1;
        tokio::time::sleep(Duration::from_millis(cfg.poll_ms)).await;
    }
}

/// Reads and checks the topic. Refuses to run without one: silent alerts are
/// the failure this tool exists to prevent.
fn read_topic() -> Result<String, String> {
    let raw = std::env::var(TOPIC_ENV)
        .map_err(|_| format!("{TOPIC_ENV} is not set (or use --dry-run)"))?;
    notify::valid_topic(&raw).map(str::to_owned)
}

/// Says, in the log, what this run can and cannot see.
fn note_startup(cfg: &Config, samples: &[Sample]) {
    let mode = if cfg.dry_run { "dry-run" } else { "live" };
    append_log(
        &cfg.log,
        &format!("{} START {mode} nodes={}", unix_now(), cfg.nodes.len()),
    );
    let mempool_seen = samples
        .iter()
        .any(|s| s.state.as_ref().is_ok_and(|o| o.pending.is_some()));
    if !mempool_seen {
        append_log(
            &cfg.log,
            "NOTE stranding detector idle: no node serves get_mempool_info",
        );
    }
}

/// Sends or prints one event, rate-limited, and logs every outcome.
async fn deliver(
    cfg: &Config,
    http: &reqwest::Client,
    topic: Option<&str>,
    rate: &mut RateLimit,
    now: u64,
    event: &Event,
) {
    let msg = notify::render(event);
    let unix = unix_now();
    let raised = matches!(event, Event::Raised(_));
    let kind = if raised { "RAISED" } else { "RESOLVED" };
    // Resolves always go out: they are bounded by the raises before them.
    if raised && !rate.allow(now) {
        append_log(
            &cfg.log,
            &format!("{unix} SUPPRESSED {} :: {}", msg.title, msg.body),
        );
        return;
    }
    append_log(
        &cfg.log,
        &format!(
            "{unix} {kind} P{} {} :: {}",
            msg.priority, msg.title, msg.body
        ),
    );
    match topic {
        None => println!("[dry-run] P{} {} :: {}", msg.priority, msg.title, msg.body),
        Some(t) => {
            if let Err(e) = notify::send(http, notify::NTFY_BASE, t, &msg).await {
                append_log(
                    &cfg.log,
                    &format!("{unix} SEND_FAILED {} :: {e}", msg.title),
                );
            }
        }
    }
}

/// Appends one line. A log that cannot be written is reported on stderr, not
/// swallowed, and the watchdog keeps running.
fn append_log(path: &Path, line: &str) {
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = written {
        eprintln!("maya2c-sentinel: cannot write {}: {e}", path.display());
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
