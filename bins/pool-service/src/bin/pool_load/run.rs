//! The orchestrator: starts the pool, connects the miners, drives the phases,
//! and prints what was measured.

use std::error::Error;
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use custom_l1_node::crypto::dag::DAG_ACTIVATION_HEIGHT;
use custom_l1_node::crypto::dag::registry::{CacheRegistry, DagConfig};
use maya_pool_service::job::Job;
use tokio::sync::watch;

use crate::miner::{MinerSpec, Phase, Tally, run_miner};
use crate::serve;

/// Settings from the command line.
pub struct Settings {
    pub miners: usize,
    pub per_miner_rate: f64,
    pub duration_secs: u64,
    pub invalid_ratio: f64,
    pub validators: usize,
    pub connect_batch: usize,
    pub job_push: bool,
    pub ledger_dir: PathBuf,
}

/// Output of the child process's stdout, parsed on a reader thread.
struct ServerFeed {
    listen: mpsc::Receiver<SocketAddr>,
    rss_last: Arc<AtomicUsize>,
    rss_peak: Arc<AtomicUsize>,
    jobs_sent: mpsc::Receiver<u32>,
}

/// Spawns `serve` as a child and starts parsing its stdout.
fn spawn_server(settings: &Settings) -> Result<(std::process::Child, ServerFeed), Box<dyn Error>> {
    let exe = std::env::current_exe()?;
    let mut child = Command::new(exe)
        .args([
            "serve",
            "--validators",
            &settings.validators.to_string(),
            "--ledger-dir",
        ])
        .arg(&settings.ledger_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;

    let stdout = child.stdout.take().ok_or("no child stdout")?;
    let (listen_tx, listen) = mpsc::channel();
    let (jobs_tx, jobs_sent) = mpsc::channel();
    let rss_last = Arc::new(AtomicUsize::new(0));
    let rss_peak = Arc::new(AtomicUsize::new(0));
    let (last, peak) = (Arc::clone(&rss_last), Arc::clone(&rss_peak));

    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(addr) = line.strip_prefix("LISTEN ") {
                if let Ok(addr) = addr.parse() {
                    let _ = listen_tx.send(addr);
                }
            } else if let Some(bytes) = line.strip_prefix("RSS ") {
                if let Ok(bytes) = bytes.parse::<usize>() {
                    last.store(bytes, Ordering::SeqCst);
                    peak.fetch_max(bytes, Ordering::SeqCst);
                }
            } else if let Some(id) = line.strip_prefix("JOBSENT ") {
                if let Ok(id) = id.parse() {
                    let _ = jobs_tx.send(id);
                }
            }
        }
    });

    Ok((
        child,
        ServerFeed {
            listen,
            rss_last,
            rss_peak,
            jobs_sent,
        },
    ))
}

/// Runs one measurement and prints the report.
pub async fn execute(settings: Settings) -> Result<(), Box<dyn Error>> {
    if settings.miners == 0 || settings.per_miner_rate <= 0.0 || settings.duration_secs == 0 {
        return Err("--miners, --per-miner-rate and --duration must all be positive".into());
    }
    if !(0.0..1.0).contains(&settings.invalid_ratio) {
        return Err("--invalid-ratio must be in [0, 1)".into());
    }

    let (mut child, feed) = spawn_server(&settings)?;
    let addr = feed
        .listen
        .recv_timeout(Duration::from_secs(120))
        .map_err(|_| "the pool did not start listening")?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let baseline = feed.rss_last.load(Ordering::SeqCst);
    feed.rss_peak.store(0, Ordering::SeqCst);

    // The same template the server holds as job 0, built here for solving.
    let dag = Arc::new(CacheRegistry::new(DagConfig::MAINNET));
    dag.cache_for_height(DAG_ACTIVATION_HEIGHT)?;
    let job = Arc::new(Job::from_candidate(0, &serve::candidate(0))?);

    let started = Instant::now();
    let events = (settings.per_miner_rate * settings.duration_secs as f64).ceil().max(1.0) as usize;
    let interval = Duration::from_secs_f64(1.0 / settings.per_miner_rate);
    let (phase_tx, phase_rx) = watch::channel(Phase::Wait);
    let ready = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));
    let mut tallies = Vec::with_capacity(settings.miners);
    let mut handles = Vec::with_capacity(settings.miners);

    for index in 0..settings.miners {
        let tally = Arc::new(Mutex::new(Tally::default()));
        tallies.push(Arc::clone(&tally));
        let spec = MinerSpec {
            index,
            addr,
            job: Arc::clone(&job),
            dag: Arc::clone(&dag),
            events,
            interval,
            invalid_ratio: settings.invalid_ratio,
        };
        let (ready, failed, rx) = (Arc::clone(&ready), Arc::clone(&failed), phase_rx.clone());
        handles.push(tokio::spawn(async move {
            if let Err(error) = run_miner(spec, rx, ready, tally).await {
                failed.fetch_add(1, Ordering::SeqCst);
                if index < 5 {
                    eprintln!("pool-load: miner {index}: {error}");
                }
            }
        }));
        if (index + 1) % settings.connect_batch == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    // Connected and solved, or failed. Failures are counted, not hidden.
    while ready.load(Ordering::SeqCst) + failed.load(Ordering::SeqCst) < settings.miners {
        if started.elapsed() > Duration::from_secs(900) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let setup_secs = started.elapsed().as_secs_f64();
    let active = ready.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_secs(1)).await;
    let idle = feed.rss_last.load(Ordering::SeqCst);
    let idle_peak = feed.rss_peak.swap(0, Ordering::SeqCst);

    let load_start = Instant::now() + Duration::from_millis(500);
    phase_tx.send_replace(Phase::Load(load_start));
    tokio::time::sleep_until(tokio::time::Instant::from_std(
        load_start + Duration::from_secs(settings.duration_secs) + Duration::from_millis(500),
    ))
    .await;
    let load_peak = feed.rss_peak.swap(0, Ordering::SeqCst);
    phase_tx.send_replace(Phase::Drain);
    tokio::time::sleep(Duration::from_secs(2)).await;

    let job_fanout = if settings.job_push {
        Some(push_job_and_wait(&mut child, &feed, &phase_tx, &tallies, active).await?)
    } else {
        None
    };

    phase_tx.send_replace(Phase::Done);
    for handle in handles {
        let _ = handle.await;
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&settings.ledger_dir);

    let report = Report {
        settings: &settings,
        baseline,
        idle,
        idle_peak,
        load_peak,
        setup_secs,
        active,
        failed: failed.load(Ordering::SeqCst),
        planned_per_miner: events,
        job_fanout,
        tallies: tallies.iter().map(|t| t.lock().map(|t| t.clone()).unwrap_or_default()).collect(),
    };
    report.print();
    Ok(())
}

/// Broadcasts one new job and measures how long each miner takes to receive it.
async fn push_job_and_wait(
    child: &mut std::process::Child,
    feed: &ServerFeed,
    phase_tx: &watch::Sender<Phase>,
    tallies: &[Arc<Mutex<Tally>>],
    active: usize,
) -> Result<Vec<f64>, Box<dyn Error>> {
    use std::io::Write;
    let stdin = child.stdin.as_mut().ok_or("no child stdin")?;
    let pushed_at = Instant::now();
    stdin.write_all(b"JOB\n")?;
    stdin.flush()?;
    let id = feed
        .jobs_sent
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| "the pool did not confirm the job push")?;
    phase_tx.send_replace(Phase::JobPush(pushed_at));

    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let seen = tallies
            .iter()
            .filter(|t| t.lock().map(|t| t.job_seen_at.is_some()).unwrap_or(false))
            .count();
        if seen >= active || Instant::now() > deadline {
            eprintln!("pool-load: job {id} seen by {seen} of {active} miners");
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    Ok(tallies
        .iter()
        .filter_map(|t| t.lock().ok().and_then(|t| t.job_seen_at))
        .map(|seen| seen.saturating_duration_since(pushed_at).as_secs_f64() * 1e3)
        .collect())
}

/// What one run measured, ready to print.
struct Report<'a> {
    settings: &'a Settings,
    baseline: usize,
    idle: usize,
    idle_peak: usize,
    load_peak: usize,
    setup_secs: f64,
    active: usize,
    failed: usize,
    planned_per_miner: usize,
    job_fanout: Option<Vec<f64>>,
    tallies: Vec<Tally>,
}

impl Report<'_> {
    fn print(&self) {
        let s = self.settings;
        let accepted: u64 = self.tallies.iter().map(|t| t.accepted).sum();
        let rejected: u64 = self.tallies.iter().map(|t| t.rejected).sum();
        let valid_rejected: u64 = self.tallies.iter().map(|t| t.valid_rejected).sum();
        let mut errs: Vec<u32> = self.tallies.iter().flat_map(|t| t.rejected_latency_us.iter().copied()).collect();
        let mut acks: Vec<u32> = self.tallies.iter().flat_map(|t| t.accepted_latency_us.iter().copied()).collect();
        let early = self.tallies.iter().filter(|t| t.ended_early).count();
        let per_conn_kib = if self.active > 0 && self.idle > self.baseline {
            (self.idle - self.baseline) as f64 / self.active as f64 / 1024.0
        } else {
            0.0
        };
        let offered = self.active as f64 * self.planned_per_miner as f64;

        println!("== pool-load result ==");
        println!("host: {} logical cores, ephemeral ports 1024-65535", std::thread::available_parallelism().map_or(0, |n| n.get()));
        println!("miners requested: {}  channels open: {}  failed: {}  ended early: {early}", s.miners, self.active, self.failed);
        println!("setup (connect, handshake, open, solve) wall: {:.1} s", self.setup_secs);
        println!("config: per-miner rate {:.3}/s, duration {} s, invalid ratio {:.3}, validators {}", s.per_miner_rate, s.duration_secs, s.invalid_ratio, s.validators);
        println!("offered shares: {offered:.0} ({} per miner)", self.planned_per_miner);
        println!("accepted: {accepted}  refused: {rejected} (of which valid refused: {valid_rejected})");
        println!("accepted shares/s over the send window: {:.1}", accepted as f64 / s.duration_secs as f64);
        println!("refused-share ack latency (immediate): p50 {} us  p99 {} us  n={}", pct(&mut errs, 0.50), pct(&mut errs, 0.99), errs.len());
        println!("accepted-share ack latency (batched, ACK_BATCH=8): p50 {} us  p99 {} us  n={}", pct(&mut acks, 0.50), pct(&mut acks, 0.99), acks.len());
        println!("server RSS (working set): baseline {} MiB, idle with {} channels {} MiB, peak idle {} MiB, peak under load {} MiB",
            mib(self.baseline), self.active, mib(self.idle), mib(self.idle_peak), mib(self.load_peak));
        println!("server memory per open channel (idle delta / channels): {per_conn_kib:.1} KiB");
        if let Some(fanout) = &self.job_fanout {
            let mut f = fanout.clone();
            f.sort_by(|a, b| a.total_cmp(b));
            println!("job fan-out (push to NewMiningJob received): p50 {:.1} ms  p99 {:.1} ms  max {:.1} ms  miners {}",
                pct_f(&f, 0.50), pct_f(&f, 0.99), f.last().copied().unwrap_or(0.0), f.len());
        }
    }
}

fn mib(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn pct(values: &mut [u32], p: f64) -> u32 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let rank = ((p * values.len() as f64).ceil() as usize).clamp(1, values.len());
    values[rank - 1]
}

fn pct_f(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}
