//! Runs one `maya2c-gpu-miner --mine --json` child per device (the CPU, or
//! one graphics card), keeps what each reports, and supervises them with the
//! rules in `watchdog.rs` (ADR-073): restart on exit with backoff, stall and
//! thermal stops, and an alert for each.
//!
//! The hub never hashes or signs: the miner binary owns the payout key file,
//! and the hub only reads the address it announces.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::miner_guard::{self, PINNED_SUMS, PinnedMiner};
use crate::pools;
use crate::watchdog::{self, AfterExit, Supervisor, Thermal};

/// Hash-rate samples kept: 15 minutes at one per two seconds.
const RATE_HISTORY: usize = 450;
/// Share and log events kept.
const EVENT_HISTORY: usize = 200;
/// Alerts kept, newest first on the wire.
const ALERT_HISTORY: usize = 100;
/// Most CPU threads the hub will ask for.
const MAX_THREADS: usize = 256;
/// Highest GPU index the hub will pass to the miner.
const MAX_DEVICE: u32 = 15;
/// Thermal ceiling the user may set, in °C.
const MIN_CEILING_C: f64 = 50.0;
const MAX_CEILING_C: f64 = 95.0;

fn default_ceiling() -> f64 {
    watchdog::THERMAL_CEILING_C
}

/// What the user chose for one device on the mining page.
#[derive(Clone, Debug, Deserialize)]
pub struct MinerConfig {
    pub binary: String,
    /// An id from `pools.rs`; the hub never takes a URL on this path.
    pub pool: String,
    pub key_file: String,
    pub threads: usize,
    /// GPU index to hash on; `None` hashes on CPU threads.
    #[serde(default)]
    pub device: Option<u32>,
    /// Stop this GPU above this temperature (°C), restart 5 °C below.
    #[serde(default = "default_ceiling")]
    pub thermal_ceiling_c: f64,
}

/// One point on the hash-rate chart.
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct RateSample {
    pub t_ms: u64,
    pub hps: f64,
}

/// Everything one worker has said since its session started.
#[derive(Clone, Debug, Default, Serialize)]
pub struct MinerState {
    /// `cpu` or `gpu<index>`.
    pub device: String,
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
    /// Automatic restarts since the session began.
    pub restarts: u32,
    /// The GPU is above its ceiling and the worker is held stopped.
    pub held: bool,
    /// Latest GPU temperature this worker's device reported.
    pub temp_c: Option<f64>,
    pub ceiling_c: Option<f64>,
    pub rates: VecDeque<RateSample>,
    pub events: VecDeque<Value>,
    /// Time of the last line from the miner, for the stall check.
    #[serde(skip)]
    last_event_ms: u64,
    /// True once this process has reported a hash rate; stalls count only then,
    /// since a GPU can take a long time to load before its first hash.
    #[serde(skip)]
    hashing: bool,
}

/// One thing the watchdog or the miner wants the user to know.
#[derive(Clone, Debug, Serialize)]
pub struct Alert {
    pub t_ms: u64,
    pub device: String,
    /// `info`, `warn` or `critical`.
    pub level: &'static str,
    pub message: String,
}

/// The whole mining page's data: each worker, and the alert log.
#[derive(Clone, Debug, Default, Serialize)]
pub struct MinerReport {
    pub workers: Vec<MinerState>,
    pub alerts: Vec<Alert>,
}

/// The running child and the read-only handle on its file.
struct Running {
    child: Child,
    _miner: PinnedMiner,
}

/// What a worker needs to start its process again without the UI.
#[derive(Clone, Debug)]
struct Plan {
    rpc: String,
    binary: String,
    install: PathBuf,
    key_file: PathBuf,
    threads: usize,
    device: Option<u32>,
    ceiling_c: f64,
}

/// One device's process, its state, and its restart bookkeeping.
struct Worker {
    plan: Plan,
    child: Option<Running>,
    state: Arc<Mutex<MinerState>>,
    watch: Supervisor,
}

#[derive(Default)]
struct Inner {
    workers: BTreeMap<String, Worker>,
    alerts: VecDeque<Alert>,
}

/// All workers and the alert log. Shared with the supervisor thread.
#[derive(Default)]
pub struct Miner {
    inner: Mutex<Inner>,
}

/// Milliseconds since the Unix epoch, the clock every event is stamped with.
#[must_use]
pub fn now_ms() -> u64 {
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

/// The worker key for a device: `cpu` or `gpu<index>`.
#[must_use]
pub fn device_key(device: Option<u32>) -> String {
    device.map_or_else(|| "cpu".to_string(), |i| format!("gpu{i}"))
}

/// Folds one event line into `state`. Unknown events are kept in the log;
/// a line that is not JSON is kept as text.
pub fn ingest(state: &mut MinerState, line: &str, t_ms: u64) {
    state.last_event_ms = t_ms;
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
            state.hashing = true;
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

/// Reads one child stream into `state` on its own thread.
fn read_lines(stream: impl std::io::Read + Send + 'static, state: Arc<Mutex<MinerState>>) {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            let mut s = state.lock().unwrap_or_else(PoisonError::into_inner);
            ingest(&mut s, &line, now_ms());
        }
    });
}

fn lock_state(state: &Mutex<MinerState>) -> std::sync::MutexGuard<'_, MinerState> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn clamp_ceiling(c: f64) -> f64 {
    if c.is_finite() {
        c.clamp(MIN_CEILING_C, MAX_CEILING_C)
    } else {
        watchdog::THERMAL_CEILING_C
    }
}

impl Worker {
    fn new(plan: Plan, device: String) -> Self {
        let state = MinerState {
            device,
            ceiling_c: plan.device.map(|_| plan.ceiling_c),
            ..MinerState::default()
        };
        Self {
            plan,
            child: None,
            state: Arc::new(Mutex::new(state)),
            watch: Supervisor::default(),
        }
    }

    fn device_args(&self) -> Vec<String> {
        self.plan.device.map_or_else(Vec::new, |i| {
            vec!["--gpu".into(), "--device".into(), i.to_string()]
        })
    }

    /// Starts the process. Used by the first start and by every restart, so
    /// the pinned binary and the key file are checked each time.
    fn launch(&mut self, now: u64) -> Result<(), String> {
        let pinned = miner_guard::check_miner(&self.plan.binary, &self.plan.install, PINNED_SUMS)
            .map_err(|e| e.to_string())?;
        let mut child = Command::new(&pinned.path)
            .args(["--mine", "--json", "--rpc"])
            .arg(&self.plan.rpc)
            .arg("--threads")
            .arg(self.plan.threads.to_string())
            .arg("--key-file")
            .arg(&self.plan.key_file)
            .args(self.device_args())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start the miner: {e}"))?;
        if let Some(out) = child.stdout.take() {
            read_lines(out, Arc::clone(&self.state));
        }
        if let Some(err) = child.stderr.take() {
            read_lines(err, Arc::clone(&self.state));
        }
        {
            let mut st = lock_state(&self.state);
            st.running = true;
            st.hashing = false;
            st.last_event_ms = now;
            st.status = "starting".into();
        }
        // The pinned miner (Windows: locked install file; Unix: private copy)
        // lives as long as the child, so what runs is what was hashed.
        self.child = Some(Running {
            child,
            _miner: pinned,
        });
        Ok(())
    }

    /// Kills the process if it runs. Never a restart: the caller decides that.
    fn halt(&mut self) {
        if let Some(mut running) = self.child.take() {
            let _ = running.child.kill();
            let _ = running.child.wait();
        }
        lock_state(&self.state).running = false;
    }

    /// The process has ended. Schedules the next start, or gives up.
    fn on_exited(&mut self, now: u64, device: &str, reason: &str) -> Alert {
        let mut st = lock_state(&self.state);
        st.running = false;
        let (level, message) = match self.watch.on_exit(now) {
            AfterExit::RestartAt(at) => {
                let secs = at.saturating_sub(now) / 1000;
                st.status = format!("{reason}; restarting in {secs}s");
                ("warn", format!("{device} {reason}; restarting in {secs} s"))
            }
            AfterExit::GiveUp => {
                st.status = format!("stopped: {} exits in 10 min", watchdog::CRASH_LIMIT);
                (
                    "critical",
                    format!(
                        "{device} {reason}; stopped after {} exits in 10 min. Start it again by hand once the cause is fixed.",
                        watchdog::CRASH_LIMIT
                    ),
                )
            }
        };
        Alert {
            t_ms: now,
            device: device.into(),
            level,
            message,
        }
    }

    /// One supervision step. Returns the alerts it raised.
    fn tick(&mut self, now: u64, device: &str, temp: Option<f64>) -> Vec<Alert> {
        let mut alerts = Vec::new();
        if let Some(t) = temp {
            lock_state(&self.state).temp_c = Some(t);
            let ceiling = self.plan.ceiling_c;
            match self.watch.on_thermal(Some(t), ceiling, now) {
                Thermal::Stop => {
                    self.halt();
                    let mut st = lock_state(&self.state);
                    st.held = true;
                    st.status = format!(
                        "thermal stop at {t:.0} °C; restarts at {:.0} °C",
                        ceiling - watchdog::THERMAL_HYSTERESIS_C
                    );
                    alerts.push(Alert { t_ms: now, device: device.into(), level: "warn", message: format!("{device} reached {t:.0} °C, above its {ceiling:.0} °C ceiling, and was stopped") });
                }
                Thermal::Resume => {
                    lock_state(&self.state).held = false;
                    alerts.push(Alert {
                        t_ms: now,
                        device: device.into(),
                        level: "info",
                        message: format!("{device} cooled to {t:.0} °C; restarting"),
                    });
                }
                Thermal::Unchanged => {}
            }
        }
        if self.child.is_some() {
            alerts.extend(self.check_running(now, device));
        } else if self.watch.take_due(now) {
            alerts.extend(self.restart(now, device));
        }
        alerts
    }

    fn check_running(&mut self, now: u64, device: &str) -> Option<Alert> {
        let ended = self
            .child
            .as_mut()
            .and_then(|r| r.child.try_wait().ok().flatten());
        if let Some(status) = ended {
            self.child = None;
            return Some(self.on_exited(now, device, &format!("exited ({status})")));
        }
        let silent = {
            let st = lock_state(&self.state);
            st.hashing && watchdog::stalled(st.last_event_ms, now)
        };
        if silent {
            self.halt();
            return Some(self.on_exited(now, device, "stalled: no output for 60 s"));
        }
        None
    }

    fn restart(&mut self, now: u64, device: &str) -> Option<Alert> {
        match self.launch(now) {
            Ok(()) => {
                lock_state(&self.state).restarts += 1;
                Some(Alert {
                    t_ms: now,
                    device: device.into(),
                    level: "info",
                    message: format!("{device} restarted"),
                })
            }
            Err(e) => Some(self.on_exited(now, device, &format!("would not start ({e})"))),
        }
    }
}

impl Miner {
    /// Starts one device's worker, replacing that device's previous session.
    /// `install` and `data` are the Hub's install and data folders; the miner
    /// and the key file are checked against them (miner_guard.rs).
    ///
    /// # Errors
    ///
    /// Already running, a bad setting, a failed check, or a binary that will not start.
    pub fn start(&self, config: &MinerConfig, install: &Path, data: &Path) -> Result<(), String> {
        let rpc = pools::resolve(&config.pool)?;
        let rpc = crate::node::check_url(rpc)?.to_string();
        if config.device.is_some_and(|i| i > MAX_DEVICE) {
            return Err(format!("GPU index above {MAX_DEVICE} is not supported"));
        }
        miner_guard::check_miner(&config.binary, install, PINNED_SUMS)
            .map_err(|e| e.to_string())?;
        let key_file =
            miner_guard::resolve_key_file(&config.key_file, data).map_err(|e| e.to_string())?;
        let plan = Plan {
            rpc,
            binary: config.binary.clone(),
            install: install.to_path_buf(),
            key_file,
            threads: config.threads.clamp(1, MAX_THREADS),
            device: config.device,
            ceiling_c: clamp_ceiling(config.thermal_ceiling_c),
        };
        let key = device_key(plan.device);
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.workers.get(&key).is_some_and(|w| w.child.is_some()) {
            return Err(format!("{key} is already mining"));
        }
        let payout_exists = plan.key_file.exists();
        let another_running = inner.workers.values().any(|w| w.child.is_some());
        if !payout_exists && another_running {
            return Err("start the first device on its own, so the payout key is created before the others read it".into());
        }
        let mut worker = inner
            .workers
            .remove(&key)
            .unwrap_or_else(|| Worker::new(plan.clone(), key.clone()));
        worker.plan = plan;
        worker.watch.reset();
        let ceiling = worker.plan.device.map(|_| worker.plan.ceiling_c);
        {
            let mut st = lock_state(&worker.state);
            let device = std::mem::take(&mut st.device);
            *st = MinerState {
                device,
                ceiling_c: ceiling,
                started_ms: Some(now_ms()),
                ..MinerState::default()
            };
        }
        let now = now_ms();
        let result = worker.launch(now);
        if let Err(reason) = &result {
            lock_state(&worker.state).status.clone_from(reason);
        }
        inner.workers.insert(key, worker);
        result
    }

    /// Stops one device's worker, or every worker when `device` is `None`.
    /// A deliberate stop is never restarted.
    pub fn stop(&self, device: Option<&str>) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        for (key, worker) in &mut inner.workers {
            if device.is_some_and(|d| d != key) {
                continue;
            }
            worker.halt();
            worker.watch.reset();
            let mut st = lock_state(&worker.state);
            st.held = false;
            st.hashing = false;
            st.status = "stopped".into();
        }
    }

    /// Runs one supervision step for every worker. `temps` holds each GPU's
    /// reading as `(index, °C)`; a GPU with no reading is never stopped.
    pub fn tick(&self, now: u64, temps: &[(u32, f64)]) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let raised: Vec<Alert> = inner
            .workers
            .iter_mut()
            .flat_map(|(key, worker)| {
                let temp = worker
                    .plan
                    .device
                    .and_then(|i| temps.iter().find(|(d, _)| *d == i).map(|(_, t)| *t));
                worker.tick(now, key, temp)
            })
            .collect();
        for alert in raised {
            push(&mut inner.alerts, alert, ALERT_HISTORY);
        }
    }

    /// True when a GPU worker runs or is held hot, so the supervisor keeps
    /// reading temperatures. A held worker has no process, and it still needs
    /// a reading to know when it may restart.
    #[must_use]
    pub fn gpu_running(&self) -> bool {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner
            .workers
            .values()
            .any(|w| w.plan.device.is_some() && (w.child.is_some() || w.watch.thermal_held()))
    }

    /// Every worker's state, and the alerts newest first.
    pub fn report(&self) -> MinerReport {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        MinerReport {
            workers: inner
                .workers
                .values()
                .map(|w| lock_state(&w.state).clone())
                .collect(),
            alerts: inner.alerts.iter().rev().cloned().collect(),
        }
    }
}

/// An amount from the miner's event stream: a decimal string (the node's
/// 256-bit form, ADR-046) or a JSON number from an older miner. Read into
/// `u128` for the dashboard, saturating past it: these figures are shown,
/// never spent.
fn units(value: &serde_json::Value) -> Option<u128> {
    match value {
        serde_json::Value::String(text)
            if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Some(text.parse::<u128>().unwrap_or(u128::MAX))
        }
        other => other.as_u64().map(u128::from),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(device: Option<u32>) -> MinerConfig {
        MinerConfig {
            // The platform's own file name: a `.exe` name is not the miner on Linux.
            binary: format!("Z:/nope/{}", miner_guard::miner_file_name()),
            pool: "public".into(),
            key_file: "k".into(),
            threads: 2,
            device,
            thermal_ceiling_c: 80.0,
        }
    }

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
    fn any_line_from_the_miner_counts_as_proof_of_life_and_a_hash_rate_arms_the_stall_check() {
        let mut s = MinerState::default();
        ingest(&mut s, r#"{"event":"idle","reason":"waiting"}"#, 500);
        assert_eq!(s.last_event_ms, 500);
        assert!(!s.hashing, "no hash rate yet, so no stall check");
        ingest(&mut s, r#"{"event":"hashrate","hps":1.0,"total":1}"#, 900);
        assert!(s.hashing);
        assert_eq!(s.last_event_ms, 900);
    }

    #[test]
    fn device_keys_and_args_name_one_gpu_or_the_cpu() {
        assert_eq!(device_key(None), "cpu");
        assert_eq!(device_key(Some(1)), "gpu1");
        let cpu = Worker::new(plan(None), "cpu".into());
        assert!(cpu.device_args().is_empty());
        let gpu = Worker::new(plan(Some(2)), "gpu2".into());
        assert_eq!(gpu.device_args(), ["--gpu", "--device", "2"]);
    }

    #[test]
    fn the_ceiling_is_clamped_and_a_nan_gets_the_default() {
        assert_eq!(clamp_ceiling(10.0), MIN_CEILING_C);
        assert_eq!(clamp_ceiling(200.0), MAX_CEILING_C);
        assert_eq!(clamp_ceiling(f64::NAN), watchdog::THERMAL_CEILING_C);
    }

    fn plan(device: Option<u32>) -> Plan {
        Plan {
            rpc: "https://rpc.maya2c.dev/rpc".into(),
            binary: "maya2c-gpu-miner.exe".into(),
            install: PathBuf::from("."),
            key_file: PathBuf::from("miner.key"),
            threads: 2,
            device,
            ceiling_c: 80.0,
        }
    }

    #[test]
    fn a_missing_binary_is_refused_before_anything_runs() {
        let miner = Miner::default();
        let temp = std::env::temp_dir();
        let err = miner.start(&config(None), &temp, &temp).unwrap_err();
        assert!(err.contains("not found"), "{err}");
        assert!(miner.report().workers.is_empty());
    }

    #[test]
    fn an_unlisted_pool_is_refused_before_the_binary_is_looked_at() {
        let miner = Miner::default();
        let temp = std::env::temp_dir();
        let mut cfg = config(None);
        cfg.pool = "http://127.0.0.1:8545".into();
        let err = miner.start(&cfg, &temp, &temp).unwrap_err();
        assert!(err.contains("unknown pool"), "{err}");
    }
}
