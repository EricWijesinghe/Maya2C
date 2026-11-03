//! Validator process management

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::config::{LabConfig, RestartPolicy, ValidatorConfig};

/// Manages validator processes
#[derive(Clone)]
pub struct ValidatorManager {
    config: Arc<LabConfig>,
    processes: Arc<Mutex<HashMap<String, ValidatorProcess>>>,
    binary_path: PathBuf,
}

#[derive(Debug)]
struct ValidatorProcess {
    name: String,
    child: Option<Child>,
    start_time: Option<Instant>,
    restart_count: u32,
    last_restart: Option<Instant>,
}

impl ValidatorManager {
    pub fn new(config: Arc<LabConfig>, binary_path: PathBuf) -> Self {
        Self {
            config,
            processes: Arc::new(Mutex::new(HashMap::new())),
            binary_path,
        }
    }

    /// Start a validator by name
    pub async fn start(&self, name: &str, catch_up_from: Option<String>) -> Result<()> {
        let validator = self
            .config
            .get_validator(name)
            .ok_or_else(|| anyhow::anyhow!("Validator not found: {}", name))?;

        info!("Starting validator: {}", name);

        // Ensure data directory exists
        std::fs::create_dir_all(&validator.data_dir)
            .with_context(|| format!("Creating data dir {}", validator.data_dir))?;

        // Build command
        let mut cmd = Command::new(&self.binary_path);
        cmd.arg("--genesis")
            .arg(&validator.genesis_path)
            .arg("--data-dir")
            .arg(&validator.data_dir)
            .arg("--rpc-addr")
            .arg(format!("127.0.0.1:{}", validator.rpc_port))
            .arg("--p2p-port")
            .arg(validator.p2p_port.to_string())
            .arg("--validator-key")
            .arg(&validator.validator_key_path)
            .arg("--metrics-addr")
            .arg(format!("127.0.0.1:{}", validator.metrics_port));

        // Every node in this lab is on 127.0.0.1; discovery needs those addresses.
        cmd.arg("--allow-private-peers");

        // Add bootnodes (all other validators + bootnode)
        for v in &self.config.validators {
            if v.name != name {
                cmd.arg("--bootnode").arg(v.p2p_multiaddr());
            }
        }
        // Add bootnode
        if let Some(bootnode) = self.config.infrastructure.bootnode.p2p_port {
            cmd.arg("--bootnode")
                .arg(format!("/ip4/127.0.0.1/tcp/{}", bootnode));
        }

        // Add catch-up if specified. B49: the restart passes a trusted block
        // from the peer it catches up from (that peer's own tip, recent by
        // definition), so the start-up guard accepts a node behind the window.
        if let Some(from) = catch_up_from {
            let url = catch_up_url(&from);
            let trusted = fetch_trusted_block_patiently(&url).await?;
            cmd.arg("--catch-up-from").arg(&url);
            cmd.arg("--trusted-block").arg(trusted);
        }

        // Set up logging
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&validator.log_path)
            .with_context(|| format!("Opening log file {}", validator.log_path))?;

        cmd.stdout(log_file.try_clone().context("Cloning log stdout")?)
            .stderr(log_file)
            .stdin(Stdio::null());

        // Spawn process
        let child = cmd
            .spawn()
            .with_context(|| format!("Spawning validator {}", name))?;

        let pid = child.id();
        info!("Validator {} started with PID {}", name, pid);

        // Store process
        let mut processes = self.processes.lock().await;
        processes.insert(
            name.to_string(),
            ValidatorProcess {
                name: name.to_string(),
                child: Some(child),
                start_time: Some(Instant::now()),
                restart_count: 0,
                last_restart: None,
            },
        );

        Ok(())
    }

    /// Stop a validator by name
    pub async fn stop(&self, name: &str, force: bool) -> Result<()> {
        let mut processes = self.processes.lock().await;

        if let Some(proc) = processes.get_mut(name) {
            if let Some(mut child) = proc.child.take() {
                info!("Stopping validator: {} (force={})", name, force);

                if force {
                    let _ = child.kill();
                } else {
                    // On Windows, we need to use taskkill for graceful shutdown
                    #[cfg(windows)]
                    {
                        let _ = Command::new("taskkill")
                            .args(["/PID", &child.id().to_string()])
                            .output();
                    }
                    #[cfg(not(windows))]
                    {
                        let _ = child.kill();
                    }
                }

                let _ = child.wait();
                info!("Validator {} stopped", name);
            }
        } else {
            warn!("Validator {} not found in process list", name);
        }

        Ok(())
    }

    /// Restart a validator
    pub async fn restart(&self, name: &str, catch_up_from: Option<String>) -> Result<()> {
        let mut processes = self.processes.lock().await;

        if let Some(proc) = processes.get_mut(name) {
            proc.restart_count += 1;
            proc.last_restart = Some(Instant::now());

            // Check restart policy
            if let Some(validator) = self.config.get_validator(name) {
                match &validator.restart_policy {
                    RestartPolicy::Never => {
                        anyhow::bail!("Restart policy is Never for validator {}", name);
                    }
                    RestartPolicy::OnFailure { max_retries } => {
                        if proc.restart_count > *max_retries {
                            anyhow::bail!(
                                "Validator {} exceeded max restarts ({})",
                                name,
                                max_retries
                            );
                        }
                    }
                    RestartPolicy::Always => {}
                }
            }
        }

        drop(processes);
        self.stop(name, true).await?;
        tokio::time::sleep(Duration::from_secs(2)).await;
        self.start(name, catch_up_from).await?;

        Ok(())
    }

    /// Check if validator is running
    pub async fn is_running(&self, name: &str) -> bool {
        let mut processes = self.processes.lock().await;
        if let Some(proc) = processes.get_mut(name) {
            if let Some(child) = &mut proc.child {
                // Try to get exit status without blocking
                match child.try_wait() {
                    Ok(Some(_)) => false, // Process exited
                    Ok(None) => true,     // Still running
                    Err(_) => false,      // Error checking
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Get validator status
    #[allow(dead_code)]
    pub async fn status(&self, name: &str) -> Option<ValidatorStatus> {
        let mut processes = self.processes.lock().await;
        processes.get_mut(name).map(|proc| ValidatorStatus {
            name: proc.name.clone(),
            running: proc
                .child
                .as_mut()
                .is_some_and(|c| c.try_wait().ok().flatten().is_none()),
            pid: proc.child.as_ref().map(|c| c.id()),
            uptime: proc.start_time.map(|t| t.elapsed()),
            restart_count: proc.restart_count,
            last_restart: proc.last_restart.map(|t| t.elapsed()),
        })
    }

    /// Get all validator statuses
    pub async fn all_status(&self) -> Vec<ValidatorStatus> {
        let mut processes = self.processes.lock().await;
        processes
            .values_mut()
            .map(|proc| ValidatorStatus {
                name: proc.name.clone(),
                running: proc
                    .child
                    .as_mut()
                    .is_some_and(|c| c.try_wait().ok().flatten().is_none()),
                pid: proc.child.as_ref().map(|c| c.id()),
                uptime: proc.start_time.map(|t| t.elapsed()),
                restart_count: proc.restart_count,
                last_restart: proc.last_restart.map(|t| t.elapsed()),
            })
            .collect()
    }

    /// Start all validators in order
    pub async fn start_all(&self, catch_up_from: Option<String>) -> Result<()> {
        let mut validators = self.config.validators.clone();
        validators.sort_by_key(|v| v.startup_order);

        for validator in validators {
            self.start(&validator.name, catch_up_from.clone()).await?;
            // Small delay between starts
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        Ok(())
    }

    /// Stop all validators
    pub async fn stop_all(&self, force: bool) -> Result<()> {
        let names: Vec<String> = {
            let processes = self.processes.lock().await;
            processes.keys().cloned().collect()
        };

        for name in names {
            self.stop(&name, force).await?;
        }

        Ok(())
    }

    /// Get process handle for direct interaction (e.g., health checks)
    #[allow(dead_code)]
    pub async fn get_child(&self, name: &str) -> Option<Child> {
        let mut processes = self.processes.lock().await;
        processes.get_mut(name).and_then(|p| p.child.take())
    }
}

/// Validator status information
#[derive(Debug, Clone, serde::Serialize)]
pub struct ValidatorStatus {
    pub name: String,
    pub running: bool,
    pub pid: Option<u32>,
    pub uptime: Option<Duration>,
    pub restart_count: u32,
    pub last_restart: Option<Duration>,
}

/// Build command for starting a validator with all necessary args
#[allow(dead_code)]
pub fn build_validator_cmd(
    binary: &PathBuf,
    validator: &ValidatorConfig,
    bootnodes: &[String],
    catch_up_from: Option<String>,
) -> Command {
    let mut cmd = Command::new(binary);
    cmd.arg("--genesis")
        .arg(&validator.genesis_path)
        .arg("--data-dir")
        .arg(&validator.data_dir)
        .arg("--rpc-addr")
        .arg(format!("127.0.0.1:{}", validator.rpc_port))
        .arg("--p2p-port")
        .arg(validator.p2p_port.to_string())
        .arg("--validator-key")
        .arg(&validator.validator_key_path)
        .arg("--metrics-addr")
        .arg(format!("127.0.0.1:{}", validator.metrics_port));

    // Every node in this lab is on 127.0.0.1; discovery needs those addresses.
    cmd.arg("--allow-private-peers");
    for bootnode in bootnodes {
        cmd.arg("--bootnode").arg(bootnode);
    }

    if let Some(from) = catch_up_from {
        cmd.arg("--catch-up-from").arg(format!("http://{}", from));
    }

    cmd
}

/// The `--catch-up-from` URL for a peer given as `host:port` or as a full URL.
/// Callers pass both forms, and a doubled scheme is not a URL.
#[must_use]
pub fn catch_up_url(from: &str) -> String {
    if from.starts_with("http://") || from.starts_with("https://") {
        from.to_string()
    } else {
        format!("http://{from}")
    }
}

/// The `--trusted-block` value from a chain-info reply and the block its tip
/// names (B49). `None` unless that block is at the reported height and its id
/// is 64 hex characters: a wrong or partial answer is never passed on.
#[must_use]
pub fn trusted_flag(info: &serde_json::Value, block: &serde_json::Value) -> Option<String> {
    let height = info["height"].as_u64()?;
    if block["height"].as_u64()? != height {
        return None;
    }
    let id = block["header"]["id"].as_str()?;
    let well_formed = id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit());
    well_formed.then(|| format!("{height}:{id}"))
}

/// One attempt: the live node at `url` reports its tip, and the block there.
///
/// Single-source is fine here: the controller runs against our own lab nodes on
/// 127.0.0.1, so the block cannot come from an outside party. Public launchers
/// require two sources to agree, or an operator override.
async fn fetch_trusted_block(url: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let info = json_rpc(&client, url, "get_chain_info", serde_json::json!([])).await?;
    let height = info["height"]
        .as_u64()
        .with_context(|| format!("{url}: no tip height"))?;
    let block = json_rpc(
        &client,
        url,
        "get_block_by_height",
        serde_json::json!([height]),
    )
    .await?;
    trusted_flag(&info, &block)
        .with_context(|| format!("{url}: tip {height} gave no usable block id"))
}

/// Retries for up to 30 s: a peer restarted just before this call is not
/// listening yet. Fails loudly after that, rather than starting a node that
/// its guard would then refuse.
pub async fn fetch_trusted_block_patiently(url: &str) -> Result<String> {
    let started = Instant::now();
    loop {
        match fetch_trusted_block(url).await {
            Ok(flag) => return Ok(flag),
            Err(_) if started.elapsed() < Duration::from_secs(30) => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(error) => return Err(error.context("no trusted block for the restart")),
        }
    }
}

async fn json_rpc(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let request =
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let reply: serde_json::Value = client
        .post(url)
        .json(&request)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .with_context(|| format!("{url}: {method}"))?
        .json()
        .await
        .with_context(|| format!("{url}: {method} reply"))?;
    if let Some(error) = reply.get("error") {
        anyhow::bail!("{url} {method}: {error}");
    }
    Ok(reply
        .get("result")
        .cloned()
        .unwrap_or(serde_json::Value::Null))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use serde_json::json;

    fn id(digit: char) -> String {
        digit.to_string().repeat(64)
    }

    #[test]
    fn a_peer_given_with_or_without_a_scheme_gets_one_scheme() {
        // Why: scenarios passes a full URL and start() used to prefix it again.
        assert_eq!(catch_up_url("127.0.0.1:34300"), "http://127.0.0.1:34300");
        assert_eq!(
            catch_up_url("http://127.0.0.1:34300"),
            "http://127.0.0.1:34300"
        );
    }

    #[test]
    fn the_tip_block_becomes_the_trusted_flag() {
        let info = json!({ "height": 42 });
        let block = json!({ "height": 42, "header": { "id": id('a') } });
        assert_eq!(trusted_flag(&info, &block), Some(format!("42:{}", id('a'))));
    }

    #[test]
    fn a_block_at_another_height_or_a_malformed_id_is_not_passed_on() {
        let info = json!({ "height": 42 });
        assert_eq!(
            trusted_flag(&info, &json!({ "height": 41, "header": { "id": id('a') } })),
            None
        );
        assert_eq!(
            trusted_flag(&info, &json!({ "height": 42, "header": { "id": "ab" } })),
            None
        );
    }
}
