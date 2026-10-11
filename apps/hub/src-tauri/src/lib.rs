//! Maya2C Hub: the miner, the network and every app in one window.
//!
//! The webview draws; these commands do everything that touches the network,
//! the GPU driver or a process. Each blocking call runs off the UI thread.

mod gpu;
mod miner;
mod node;

use std::path::PathBuf;

use serde::Serialize;
use tauri::Manager;

use crate::miner::{Miner, MinerConfig, MinerState};

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
}

/// The public testnet RPC, so a fresh install shows a live network without
/// running a node. Settings can point it at a local one.
const PUBLIC_RPC: &str = "https://rpc.maya2c.dev/rpc";

/// The miner next to the hub's own executable, where a release bundle puts
/// it; otherwise the bare name, which the user can correct.
fn miner_beside_hub() -> String {
    let name = if cfg!(windows) {
        "maya2c-gpu-miner.exe"
    } else {
        "maya2c-gpu-miner"
    };
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.is_file())
        .map_or_else(|| name.to_string(), |path| path.display().to_string())
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
    }
}

#[tauri::command]
async fn node_snapshot(url: String, address: Option<String>) -> Result<node::Snapshot, String> {
    node::check_url(&url)?;
    tauri::async_runtime::spawn_blocking(move || node::snapshot(&url, address.as_deref()))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn gpu_telemetry() -> Result<gpu::GpuReport, String> {
    tauri::async_runtime::spawn_blocking(gpu::read)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn miner_start(config: MinerConfig, state: tauri::State<'_, Miner>) -> Result<(), String> {
    if cfg!(mobile) {
        return Err("mining runs on the desktop Hub; this one watches".into());
    }
    state.start(&config)
}

#[tauri::command]
fn miner_stop(state: tauri::State<'_, Miner>) {
    state.stop();
}

#[tauri::command]
fn miner_state(state: tauri::State<'_, Miner>) -> MinerState {
    state.state()
}

/// # Panics
///
/// If the Tauri runtime cannot start, which leaves nothing to show.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Miner::default())
        .invoke_handler(tauri::generate_handler![
            defaults,
            node_snapshot,
            gpu_telemetry,
            miner_start,
            miner_stop,
            miner_state
        ])
        .on_window_event(|window, event| {
            // A miner left running after the window closes would be a miner
            // nobody can see. Closing the hub stops it.
            if let tauri::WindowEvent::Destroyed = event {
                window.state::<Miner>().stop();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running the Maya2C Hub");
}
