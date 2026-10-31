//! Runs `maya2c-gpu-miner --mine --json` as a child process and keeps what
//! it reports, for the miner page to draw.
//!
//! The hub never hashes or signs: the miner binary owns the payout key file,
//! and the hub only reads the address it announces.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::miner_guard::{self, PINNED_SUMS};

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
    /// Hash on the graphics card (`--gpu`) instead of CPU threads.
    #[serde(default)]
    pub gpu: bool,
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
    pub share_reward: Option<u128>,
    pub accepted: u64,
    pub rejected: u64,
    /// Growth of the payout account's on-chain balance since this run's
    /// first share. Not the sum of accepted shares: the node accepting a
    /// share into its pool is not the chain paying it (measured on the local
    /// mining devnet, 145 accepted against 10 paid).
    pub earned: u128,
    #[serde(skip)]
    first_balance: Option<u128>,
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
        push(
            &mut state.events,
            serde_json::json!({"event": "log", "message": line, "t_ms": t_ms}),
            EVENT_HISTORY,
        );
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
            state.share_reward = units(&event["share_reward"]);
            state.status = "mining".into();
        }
        "idle" => {
            state.status = event["reason"]
                .as_str()
                .unwrap_or("waiting for mining to open")
                .to_string();
        }
        "hashrate" => {
            let hps = event["hps"].as_f64().unwrap_or(0.0);
            state.total_hashes = event["total"].as_u64().unwrap_or(state.total_hashes);
            push(&mut state.rates, RateSample { t_ms, hps }, RATE_HISTORY);
            return;
        }
        "share" if event["accepted"].as_bool() == Some(true) => {
            state.accepted += 1;
            if let Some(balance) = units(&event["balance"]) {
                let first = *state.first_balance.get_or_insert(balance);
                state.earned = balance.saturating_sub(first);
            }
        }
        "share" => state.rejected += 1,
        _ => {}
    }
    let mut stamped = event;
    stamped["t_ms"] = t_ms.into();
    push(&mut state.events, stamped, EVENT_HISTORY);
}

impl Miner {
    /// Starts the miner, replacing any previous run's numbers. `install` and
    /// `data` are the Hub's install and data folders; the miner and the key
    /// file are checked against them (miner_guard.rs).
    ///
    /// # Errors
    ///
    /// Already running, a bad setting, a failed check, or a binary that will not start.
    pub fn start(&self, config: &MinerConfig, install: &Path, data: &Path) -> Result<(), String> {
        crate::node::check_url(&config.rpc)?;
        let binary = miner_guard::check_miner(&config.binary, install, PINNED_SUMS)
            .map_err(|e| e.to_string())?;
        let key_file = miner_guard::resolve_key_file(&config.key_file, data)
            .map_err(|e| e.to_string())?;
        let threads = config.threads.clamp(1, MAX_THREADS);
        let mut slot = self.child.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_some() {
            return Err("the miner is already running".into());
        }
        let mut child = Command::new(&binary)
            .args(["--mine", "--json", "--rpc", &config.rpc, "--threads"])
            .arg(threads.to_string())
            .arg("--key-file")
            .arg(&key_file)
            .args(config.gpu.then_some("--gpu"))
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
        if let Some(mut child) = self
            .child
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
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

/// An amount from the miner's event stream: a decimal string (the node's
/// 256-bit form, ADR-046) or a JSON number from an older miner. Read into
/// `u128` for the dashboard, saturating past it: these figures are shown,
/// never spent.
fn units(value: &serde_json::Value) -> Option<u128> {
    match value {
        serde_json::Value::String(text) if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) => {
            Some(text.parse::<u128>().unwrap_or(u128::MAX))
        }
        other => other.as_u64().map(u128::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_build_up_the_miner_state() {
        let mut s = MinerState::default();
        ingest(
            &mut s,
            r#"{"event":"start","address":"ab","mode":"cpu-light"}"#,
            1,
        );
        ingest(
            &mut s,
            r#"{"event":"work","epoch":7,"share_reward":70000}"#,
            2,
        );
        ingest(
            &mut s,
            r#"{"event":"hashrate","hps":512.5,"total":1024}"#,
            3,
        );
        ingest(
            &mut s,
            r#"{"event":"share","accepted":true,"reward":70000,"balance":500}"#,
            4,
        );
        ingest(
            &mut s,
            r#"{"event":"share","accepted":false,"reason":"stale"}"#,
            5,
        );
        ingest(&mut s, "plain text from stderr", 6);

        assert_eq!(s.address.as_deref(), Some("ab"));
        assert_eq!(
            (s.epoch, s.accepted, s.rejected, s.earned),
            (Some(7), 1, 1, 0)
        );
        assert_eq!(
            s.rates.back(),
            Some(&RateSample {
                t_ms: 3,
                hps: 512.5
            })
        );
        assert_eq!(s.total_hashes, 1024);
        assert_eq!(
            s.events.back().unwrap()["message"],
            "plain text from stderr"
        );
    }

    #[test]
    fn earned_is_on_chain_balance_growth_not_accepted_shares() {
        let mut s = MinerState::default();
        for balance in [100, 100, 100, 170] {
            let line =
                format!(r#"{{"event":"share","accepted":true,"reward":70,"balance":{balance}}}"#);
            ingest(&mut s, &line, 1);
        }
        assert_eq!((s.accepted, s.earned), (4, 70));
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
        let temp = std::env::temp_dir();
        let err = miner
            .start(
                &MinerConfig {
                    binary: "Z:/nope/maya2c-gpu-miner.exe".into(),
                    rpc: "http://127.0.0.1:8545".into(),
                    key_file: "k".into(),
                    threads: 2,
                    gpu: false,
                },
                &temp,
                &temp,
            )
            .unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(!miner.state().running);
    }
}
