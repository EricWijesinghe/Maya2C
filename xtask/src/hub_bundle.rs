//! `cargo xtask hub-bundle`: the Maya2C Hub installer for this host, with the
//! miner inside it (ADR-045).
//!
//! The Hub starts `maya2c-gpu-miner` from beside its own executable. Tauri
//! ships a sidecar there only when the file is named for the target triple,
//! so the release miner is copied to `binaries/maya2c-gpu-miner-<triple>`
//! first. The sidecar lives in `tauri.release.conf.json`, not the main
//! config, so a plain `cargo build` or `cargo test` of the Hub never needs a
//! release miner to exist.
//!
//! Windows yields NSIS and MSI installers; Linux (run it in WSL) yields .deb,
//! .rpm and AppImage. macOS waits for a Mac to build on.

use std::fs;
use std::path::Path;
use std::process::Command;

const MINER: &str = "maya2c-gpu-miner";

pub fn run(_args: &[String]) -> Result<(), String> {
    let root = crate::workspace_root();
    let exe = std::env::consts::EXE_SUFFIX;
    status(
        Command::new("cargo")
            .args(["build", "--release", "-p", MINER, "--features", "gpu"])
            .current_dir(&root),
        "cargo build --release -p maya2c-gpu-miner --features gpu",
    )?;

    let hub = root.join("apps/hub/src-tauri");
    let triple = host_triple()?;
    let sidecars = hub.join("binaries");
    fs::create_dir_all(&sidecars).map_err(|e| format!("{}: {e}", sidecars.display()))?;
    // Honoured so a WSL build keeps its artifacts off the Windows `target/`:
    // two hosts' fingerprints in one directory corrupt each other.
    let custom = std::env::var_os("CARGO_TARGET_DIR").map(std::path::PathBuf::from);
    let target = custom.clone().unwrap_or_else(|| root.join("target"));
    let from = target.join("release").join(format!("{MINER}{exe}"));
    let to = sidecars.join(format!("{MINER}-{triple}{exe}"));
    fs::copy(&from, &to).map_err(|e| format!("{} -> {}: {e}", from.display(), to.display()))?;
    println!("sidecar: {}", to.display());

    // Release CI passes a signed-updater config here (release-binaries.yml);
    // without one the Hub is the unsigned build that never offers an update.
    let mut build = Command::new("cargo");
    build.args(["tauri", "build", "--config", "tauri.release.conf.json"]);
    if let Some(updater) = std::env::var_os("HUB_UPDATER_CONFIG") {
        build.arg("--config").arg(updater);
    }
    status(
        build.current_dir(&hub),
        "cargo tauri build (install tauri-cli 2 if missing)",
    )?;
    let hub_target = custom.unwrap_or_else(|| hub.join("target"));
    report(&hub_target.join("release/bundle"));
    Ok(())
}

fn status(cmd: &mut Command, what: &str) -> Result<(), String> {
    let ok = cmd.status().map_err(|e| format!("{what}: {e}"))?.success();
    if ok {
        Ok(())
    } else {
        Err(format!("{what} failed"))
    }
}

/// The `host:` line of `rustc -vV`, e.g. `x86_64-pc-windows-msvc`.
fn host_triple() -> Result<String, String> {
    let out = Command::new("rustc")
        .arg("-vV")
        .output()
        .map_err(|e| format!("rustc -vV: {e}"))?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(str::to_string)
        .ok_or_else(|| "rustc -vV printed no host triple".into())
}

/// Lists each installer with its size, so the run ends with what it made.
fn report(bundle: &Path) {
    let Ok(kinds) = fs::read_dir(bundle) else {
        println!("no bundle directory at {}", bundle.display());
        return;
    };
    for kind in kinds.flatten() {
        let Ok(files) = fs::read_dir(kind.path()) else {
            continue;
        };
        for file in files.flatten() {
            let size = file.metadata().map_or(0, |m| m.len());
            if file.path().is_file() {
                println!("{:>12} bytes  {}", size, file.path().display());
            }
        }
    }
}
