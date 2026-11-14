//! Maya2C Hub: the miner, the network and every app in one window.
//!
//! The webview draws; these commands do everything that touches the network,
//! the GPU driver or a process. Each blocking call runs off the UI thread.

mod gpu;
#[cfg(windows)]
mod key_acl;
mod miner;
mod miner_guard;
mod node;
mod notify;
mod pools;
mod watch;
mod watchdog;

use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde::Serialize;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

use crate::miner::{Miner, MinerConfig, MinerReport};
use crate::miner_guard::miner_file_name;
use crate::notify::Notifier;

/// Starting values for the settings panel.
#[derive(Serialize)]
struct Defaults {
    rpc: String,
    miner_binary: String,
    key_file: String,
    cpu_threads: usize,
    /// Mine on the GPU unless the user turns it off; the miner reports an
    /// error event when no adapter is found.
    use_gpu: bool,
    /// False on phones: the Hub there watches the network and your miners'
    /// payouts, and runs no miner (battery and heat for next to no hashes).
    can_mine: bool,
    /// The nodes a rig may mine through (ADR-073 decision 5).
    pools: &'static [pools::Pool],
    thermal_ceiling_c: f64,
}

/// The public testnet RPC, so a fresh install shows a live network without
/// running a node. Settings can point it at a local one.
const PUBLIC_RPC: &str = "https://rpc.maya2c.dev/rpc";

/// The miner next to the hub's own executable, where a release bundle puts
/// it; otherwise the bare name, which the user can correct.
fn miner_beside_hub() -> String {
    let name = miner_file_name();
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
        .filter(|path| path.is_file())
        .map_or(name, |path| path.display().to_string())
}

#[tauri::command]
fn defaults(app: tauri::AppHandle) -> Defaults {
    let key_file = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("miner.key");
    Defaults {
        rpc: PUBLIC_RPC.into(),
        miner_binary: miner_beside_hub(),
        key_file: key_file.display().to_string(),
        cpu_threads: std::thread::available_parallelism().map_or(4, std::num::NonZero::get),
        use_gpu: true,
        can_mine: cfg!(desktop),
        pools: pools::POOLS,
        thermal_ceiling_c: watchdog::THERMAL_CEILING_C,
    }
}

#[tauri::command]
async fn node_snapshot(url: String, address: Option<String>) -> Result<node::Snapshot, String> {
    let url = node::check_url(&url)?.to_owned();
    tauri::async_runtime::spawn_blocking(move || node::snapshot(&url, address.as_deref()))
        .await
        .map_err(|e| e.to_string())
}

/// Watch-only balances for the Wallet page: public reads, no keys (ADR-083).
#[tauri::command]
async fn watch_balances(
    url: String,
    addresses: Vec<String>,
) -> Result<Vec<watch::Watched>, String> {
    let url = node::check_url(&url)?.to_owned();
    tauri::async_runtime::spawn_blocking(move || watch::watch(&url, &addresses))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn gpu_telemetry() -> Result<gpu::GpuReport, String> {
    tauri::async_runtime::spawn_blocking(gpu::read)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn miner_start(
    config: MinerConfig,
    app: tauri::AppHandle,
    state: tauri::State<'_, Miner>,
) -> Result<(), String> {
    if cfg!(mobile) {
        return Err("mining runs on the desktop Hub; this one watches".into());
    }
    let install = miner_guard::install_dir().map_err(|e| e.to_string())?;
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    state.start(&config, &install, &data)
}

/// Turns OS toasts for alerts on or off. Off by default; the webview sends the
/// saved choice on start. Turning them on asks the OS for permission first
/// (Android needs it; desktop grants it without asking).
#[tauri::command]
fn notify_set(
    on: bool,
    app: tauri::AppHandle,
    notifier: tauri::State<'_, Mutex<Notifier>>,
) -> Result<(), String> {
    if on {
        app.notification()
            .request_permission()
            .map_err(|e| e.to_string())?;
    }
    notify::lock(&notifier).set_enabled(on);
    Ok(())
}

/// Stops one device's miner (`cpu`, `gpu0`, ...), or all of them when no
/// device is named. A stop is never followed by an automatic restart.
#[tauri::command]
fn miner_stop(device: Option<String>, state: tauri::State<'_, Miner>) {
    state.stop(device.as_deref());
}

#[tauri::command]
fn miner_state(state: tauri::State<'_, Miner>) -> MinerReport {
    state.report()
}

/// Runs the watchdog once a second. GPU temperatures are read every third
/// second and only while a GPU miner runs, so an idle Hub never calls
/// `nvidia-smi`. A tick without a reading never stops or restarts a GPU.
fn supervise(app: tauri::AppHandle) {
    let mut tick: u64 = 0;
    loop {
        thread::sleep(Duration::from_secs(1));
        tick += 1;
        let miner = app.state::<Miner>();
        let temps = if tick.is_multiple_of(3) && miner.gpu_running() {
            gpu::temperatures(&gpu::read())
        } else {
            Vec::new()
        };
        let raised = miner.tick(miner::now_ms(), &temps);
        let notifier = app.state::<Mutex<Notifier>>();
        for alert in &raised {
            if notify::lock(&notifier).admit(alert) {
                notify::show(&app, alert);
            }
        }
    }
}

/// # Panics
///
/// If the Tauri runtime cannot start, which leaves nothing to show.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(maya_app_update::dialog())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            thread::spawn(move || supervise(handle));
            Ok(maya_app_update::start(app)?)
        })
        .manage(Miner::default())
        .manage(Mutex::new(Notifier::default()))
        .invoke_handler(tauri::generate_handler![
            defaults,
            node_snapshot,
            watch_balances,
            gpu_telemetry,
            miner_start,
            miner_stop,
            miner_state,
            notify_set
        ])
        .on_window_event(|window, event| {
            // A miner left running after the window closes would be a miner
            // nobody can see. Closing the hub stops every one of them.
            if let tauri::WindowEvent::Destroyed = event {
                window.state::<Miner>().stop(None);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running the Maya2C Hub");
}
