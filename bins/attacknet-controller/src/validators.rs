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

        // Add catch-up if specified
        if let Some(from) = catch_up_from {
            cmd.arg("--catch-up-from").arg(format!("http://{}", from));
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

    for bootnode in bootnodes {
        cmd.arg("--bootnode").arg(bootnode);
    }

    if let Some(from) = catch_up_from {
        cmd.arg("--catch-up-from").arg(format!("http://{}", from));
    }

    cmd
}
