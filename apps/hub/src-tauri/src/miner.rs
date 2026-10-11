//! Runs `maya2c-gpu-miner --mine --json` as a child process and keeps what
//! it reports, for the miner page to draw.
//!
//! The hub never hashes or signs: the miner binary owns the payout key file,
//! and the hub only reads the address it announces.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Hash-rate samples kept: 15 minutes at one per two seconds.
const RATE_HISTORY: usize = 450;
/// Share and log events kept.
const EVENT_HISTORY: usize = 200;
/// Most CPU threads the hub will ask for.
const MAX_THREADS: usize = 256;

/// What the user chose on the miner page.
#[derive(Clone, Debug, Deserialize)]
pub struct MinerConfig {
    pub binary: String,
    pub rpc: String,
    pub key_file: String,
    pub threads: usize,
}

/// One point on the hash-rate chart.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct RateSample {
    pub t_ms: u64,
    pub hps: f64,
}

/// Everything the miner has said since it started.
#[derive(Clone, Debug, Default, Serialize)]
pub struct MinerState {
    pub running: bool,
    pub address: Option<String>,
    pub mode: Option<String>,
    pub epoch: Option<u64>,
    pub share_reward: Option<u64>,
    pub accepted: u64,
    pub rejected: u64,
    pub earned: u64,
    pub total_hashes: u64,
    pub started_ms: Option<u64>,
    pub status: String,
    pub rates: VecDeque<RateSample>,
    pub events: VecDeque<Value>,
}

/// The child and its state, shared with the reader thread.
#[derive(Default)]
pub struct Miner {
    child: Mutex<Option<Child>>,
    state: Arc<Mutex<MinerState>>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn push<T>(queue: &mut VecDeque<T>, item: T, cap: usize) {
    if queue.len() == cap {
        queue.pop_front();
    }
    queue.push_back(item);
}

/// Folds one event line into `state`. Unknown events are kept in the log;
/// a line that is not JSON is kept as text.
pub fn ingest(state: &mut MinerState, line: &str, t_ms: u64) {
    let Ok(event) = serde_json::from_str::<Value>(line) else {
        push(&mut state.events, serde_json::json!({"event": "log", "message": line, "t_ms": t_ms}), EVENT_HISTORY);
        return;
    };
    match event["event"].as_str().unwrap_or("") {
        "start" => {
            state.address = event["address"].as_str().map(str::to_string);
            state.mode = event["mode"].as_str().map(str::to_string);
            state.status = "connected".into();
        }
        "work" => {
            state.epoch = event["epoch"].as_u64();
            state.share_reward = event["share_reward"].as_u64();
            state.status = "mining".into();
        }
        "idle" => state.status = "waiting for mining to open".into(),
        "hashrate" => {
            let hps = event["hps"].as_f64().unwrap_or(0.0);
            state.total_hashes = event["total"].as_u64().unwrap_or(state.total_hashes);
            push(&mut state.rates, RateSample { t_ms, hps }, RATE_HISTORY);
            return;
        }
        "share" if event["accepted"].as_bool() == Some(true) => {
            state.accepted += 1;
            state.earned = state.earned.saturating_add(event["reward"].as_u64().unwrap_or(0));
        }
        "share" => state.rejected += 1,
        _ => {}
    }
    let mut stamped = event;
    stamped["t_ms"] = t_ms.into();
    push(&mut state.events, stamped, EVENT_HISTORY);
}

impl Miner {
    /// Starts the miner, replacing any previous run's numbers.
    ///
    /// # Errors
    ///
    /// Already running, a bad setting, or a binary that will not start.
    pub fn start(&self, config: &MinerConfig) -> Result<(), String> {
        crate::node::check_url(&config.rpc)?;
        let binary = PathBuf::from(&config.binary);
        if !binary.is_file() {
            return Err(format!("miner not found at {}", binary.display()));
        }
        if config.key_file.trim().is_empty() {
            return Err("choose a payout key file".into());
        }
        let threads = config.threads.clamp(1, MAX_THREADS);
        let mut slot = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return Err("the miner is already running".into());
        }
        let mut child = Command::new(&binary)
            .args(["--mine", "--json", "--rpc", &config.rpc, "--threads"])
            .arg(threads.to_string())
            .arg("--key-file")
            .arg(Path::new(&config.key_file))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start the miner: {e}"))?;
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            *state = MinerState {
                running: true,
                started_ms: Some(now_ms()),
                status: "starting".into(),
                ..MinerState::default()
            };
        }
        if let Some(out) = child.stdout.take() {
            self.read_lines(out);
        }
        if let Some(err) = child.stderr.take() {
            self.read_lines(err);
        }
        *slot = Some(child);
        Ok(())
    }

    fn read_lines(&self, stream: impl std::io::Read + Send + 'static) {
        let state = Arc::clone(&self.state);
        thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
                ingest(&mut s, &line, now_ms());
            }
        });
    }

    /// Stops the miner if it runs.
    pub fn stop(&self) {
        if let Some(mut child) = self.child.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.running = false;
        state.status = "stopped".into();
    }

    /// The state now, noticing a miner that exited on its own.
    pub fn state(&self) -> MinerState {
        let mut slot = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        let exited = slot
            .as_mut()
            .and_then(|c| c.try_wait().ok().flatten())
            .map(|status| status.to_string());
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(status) = exited {
            *slot = None;
            state.running = false;
            state.status = format!("miner exited ({status})");
        }
        state.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_build_up_the_miner_state() {
        let mut s = MinerState::default();
        ingest(&mut s, r#"{"event":"start","address":"ab","mode":"cpu-light"}"#, 1);
        ingest(&mut s, r#"{"event":"work","epoch":7,"share_reward":70000}"#, 2);
        ingest(&mut s, r#"{"event":"hashrate","hps":512.5,"total":1024}"#, 3);
        ingest(&mut s, r#"{"event":"share","accepted":true,"reward":70000}"#, 4);
        ingest(&mut s, r#"{"event":"share","accepted":false,"reason":"stale"}"#, 5);
        ingest(&mut s, "plain text from stderr", 6);

        assert_eq!(s.address.as_deref(), Some("ab"));
        assert_eq!((s.epoch, s.accepted, s.rejected, s.earned), (Some(7), 1, 1, 70_000));
        assert_eq!(s.rates.back(), Some(&RateSample { t_ms: 3, hps: 512.5 }));
        assert_eq!(s.total_hashes, 1024);
        assert_eq!(s.events.back().unwrap()["message"], "plain text from stderr");
    }

    #[test]
    fn history_is_bounded() {
        let mut s = MinerState::default();
        for t in 0..(RATE_HISTORY as u64 + 50) {
            ingest(&mut s, r#"{"event":"hashrate","hps":1.0,"total":1}"#, t);
        }
        assert_eq!(s.rates.len(), RATE_HISTORY);
        assert_eq!(s.rates.front().unwrap().t_ms, 50);
    }

    #[test]
    fn a_missing_binary_is_refused_before_anything_runs() {
        let miner = Miner::default();
        let err = miner
            .start(&MinerConfig {
                binary: "Z:/nope/maya2c-gpu-miner.exe".into(),
                rpc: "http://127.0.0.1:8545".into(),
                key_file: "k".into(),
                threads: 2,
            })
            .unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(!miner.state().running);
    }
}
