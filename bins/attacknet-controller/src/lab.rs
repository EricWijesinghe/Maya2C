//! Main lab orchestration

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::config::LabConfig;
use crate::evidence::EvidenceCollector;
use crate::fault_injection::FaultInjector;
use crate::metrics::LabMetrics;
use crate::validators::ValidatorManager;

/// Main lab coordinator
pub struct Lab {
    pub config: Arc<LabConfig>,
    pub validator_manager: ValidatorManager,
    #[expect(dead_code)]
    fault_injector: FaultInjector,
    #[expect(dead_code)]
    evidence_collector: EvidenceCollector,
    metrics: Arc<LabMetrics>,
    #[expect(dead_code)]
    processes: Arc<Mutex<Vec<tokio::process::Child>>>,
    infrastructure_processes: Arc<Mutex<HashMap<String, tokio::process::Child>>>,
}

use std::collections::HashMap;

impl Lab {
    pub async fn new(config: LabConfig) -> Result<Self> {
        let config = Arc::new(config);

        // Find the maya2c-node binary
        let binary_path = Self::find_binary("maya2c-node")?;

        let validator_manager = ValidatorManager::new(config.clone(), binary_path);
        let fault_injector = FaultInjector::new((*config).clone());
        let evidence_collector = EvidenceCollector::new(config.paths.runs_dir.clone().into());
        let metrics = Arc::new(LabMetrics::new());

        Ok(Self {
            config,
            validator_manager,
            fault_injector,
            evidence_collector,
            metrics,
            processes: Arc::new(Mutex::new(Vec::new())),
            infrastructure_processes: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn find_binary(name: &str) -> Result<PathBuf> {
        // Check MAYA2C_BIN_DIR first
        if let Ok(bin_dir) = std::env::var("MAYA2C_BIN_DIR") {
            let path = PathBuf::from(bin_dir).join(format!("{}.exe", name));
            if path.exists() {
                return Ok(path);
            }
        }

        // Check target/release
        let path = PathBuf::from("target/release").join(format!("{}.exe", name));
        if path.exists() {
            return Ok(path);
        }

        // Check target/debug
        let path = PathBuf::from("target/debug").join(format!("{}.exe", name));
        if path.exists() {
            return Ok(path);
        }

        anyhow::bail!("Binary not found: {}", name)
    }

    /// Provision the lab: create directories, generate keys, write configs
    pub async fn provision(&self) -> Result<()> {
        info!("Provisioning attacknet lab...");

        // Create all directories
        self.create_directories().await?;

        // Generate genesis
        self.generate_genesis().await?;

        // Generate validator keys
        self.generate_validator_keys().await?;

        // Write validator configs
        self.write_validator_configs().await?;

        // Write infrastructure configs
        self.write_infrastructure_configs().await?;

        // Write monitoring configs
        self.write_monitoring_configs().await?;

        info!("Lab provisioning complete");
        Ok(())
    }

    async fn create_directories(&self) -> Result<()> {
        let dirs = vec![
            &self.config.paths.base_dir,
            &self.config.paths.genesis_dir,
            &self.config.paths.reports_dir,
            &self.config.paths.runs_dir,
            &self.config.paths.control_dir,
        ];

        for dir in dirs {
            std::fs::create_dir_all(dir).with_context(|| format!("Creating directory {}", dir))?;
        }

        // Validator data directories
        for validator in &self.config.validators {
            std::fs::create_dir_all(&validator.data_dir)
                .with_context(|| format!("Creating validator data dir {}", validator.data_dir))?;
        }

        // Infrastructure directories
        std::fs::create_dir_all(&self.config.infrastructure.bootnode.data_dir)?;
        std::fs::create_dir_all(&self.config.infrastructure.rpc_gateway.data_dir)?;
        std::fs::create_dir_all(&self.config.infrastructure.adversarial_peer.data_dir)?;

        // Monitoring directories
        std::fs::create_dir_all(&self.config.monitoring.prometheus.data_dir)?;
        std::fs::create_dir_all(&self.config.monitoring.grafana.data_dir)?;
        std::fs::create_dir_all(&self.config.monitoring.alertmanager.data_dir)?;
        std::fs::create_dir_all(&self.config.monitoring.vector.data_dir)?;

        Ok(())
    }

    async fn generate_genesis(&self) -> Result<()> {
        let genesis_path = Path::new(&self.config.paths.genesis_dir).join("genesis.json");

        if genesis_path.exists() {
            info!("Genesis already exists at {}", genesis_path.display());
            return Ok(());
        }

        info!("Generating genesis...");

        // Use the genesis-ceremony binary or localnet setup
        // For now, generate a basic genesis with the 4 validators
        let mut validators = Vec::new();
        for validator in &self.config.validators {
            // Read the public key from the key file
            let key_path = Path::new(&validator.validator_key_path);
            if key_path.exists() {
                let key_content = std::fs::read_to_string(key_path)?;
                validators.push(key_content.trim().to_string());
            } else {
                // Generate a new key if it doesn't exist
                let output = tokio::process::Command::new("maya2c-node")
                    .arg("--generate-validator-key")
                    .arg(key_path)
                    .output()
                    .await
                    .context("Generating validator key")?;
                let pubkey = String::from_utf8_lossy(&output.stdout).trim().to_string();
                validators.push(pubkey);
            }
        }

        // Generate genesis using l1-wallet
        let genesis = serde_json::json!({
            "chain_id": self.config.chain_id,
            "timestamp": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            "difficulty_bits": 0,
            "pow_limit_bits": 0,
            "allocations": [{
                "address": "5a".repeat(32), // Placeholder
                "balance": 10_000_000_000u64
            }],
            "bft": {
                "validators": validators,
                "anchor_timeout_ms": 1000,
                "batch_size": 500,
                "staking": {
                    "epoch_blocks": 60,
                    "bonds": validators.iter().map(|_| serde_json::json!({
                        "operator": "5a".repeat(32),
                        "bond": 100_000_000u64
                    })).collect::<Vec<_>>(),
                    "stake_weighted": true
                }
            }
        });

        std::fs::write(&genesis_path, serde_json::to_string_pretty(&genesis)?)?;
        info!("Genesis written to {}", genesis_path.display());

        Ok(())
    }

    async fn generate_validator_keys(&self) -> Result<()> {
        for validator in &self.config.validators {
            let key_path = Path::new(&validator.validator_key_path);
            if key_path.exists() {
                debug!("Validator key already exists: {}", key_path.display());
                continue;
            }

            info!("Generating validator key for {}", validator.name);

            let output = tokio::process::Command::new("maya2c-node")
                .arg("--generate-validator-key")
                .arg(key_path)
                .output()
                .await
                .context("Generating validator key")?;

            if !output.status.success() {
                error!(
                    "Key generation failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                anyhow::bail!("Failed to generate key for {}", validator.name);
            }

            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let pubkey = stdout_str.trim();
            info!("Generated key for {}: {}", validator.name, pubkey);
        }

        Ok(())
    }

    async fn write_validator_configs(&self) -> Result<()> {
        for validator in &self.config.validators {
            let config = format!(
                r#"
[network]
p2p_port = {}
bootnodes = [
{}
]
dual_kem = "off"

[rpc]
listen = "127.0.0.1:{}"
rate_limit_per_second = 1000
rate_limit_burst = 2000

[metrics]
listen = "127.0.0.1:{}"

[storage]
block_cache_mib = 32
write_buffer_mib = 8
max_open_files = 256
"#,
                validator.p2p_port,
                self.config
                    .validators
                    .iter()
                    .filter(|v| v.name != validator.name)
                    .map(|v| format!("\"/ip4/127.0.0.1/tcp/{}\"", v.p2p_port))
                    .collect::<Vec<_>>()
                    .join(",\n"),
                validator.rpc_port,
                validator.metrics_port
            );

            std::fs::write(&validator.config_path, config)
                .with_context(|| format!("Writing config for {}", validator.name))?;
        }

        // Add bootnode to each validator's bootnodes
        for validator in &self.config.validators {
            let mut config = std::fs::read_to_string(&validator.config_path)?;
            let bootnode_line = format!(
                "\"/ip4/127.0.0.1/tcp/{}\"",
                self.config
                    .infrastructure
                    .bootnode
                    .p2p_port
                    .unwrap_or(33310)
            );
            config = config.replace(
                "bootnodes = [",
                &format!("bootnodes = [\n    {},", bootnode_line),
            );
            std::fs::write(&validator.config_path, config)?;
        }

        Ok(())
    }

    async fn write_infrastructure_configs(&self) -> Result<()> {
        // Bootnode config
        let bootnode_config = format!(
            r#"
[network]
p2p_port = {}
bootnodes = []
dual_kem = "off"

[rpc]
listen = "127.0.0.1:{}"

[metrics]
listen = "127.0.0.1:{}"
"#,
            self.config
                .infrastructure
                .bootnode
                .p2p_port
                .unwrap_or(33310),
            self.config
                .infrastructure
                .bootnode
                .rpc_port
                .unwrap_or(34310),
            self.config
                .infrastructure
                .bootnode
                .metrics_port
                .unwrap_or(35310)
        );
        std::fs::write(
            &self.config.infrastructure.bootnode.config_path,
            bootnode_config,
        )?;

        // RPC Gateway config
        let gateway_config = format!(
            r#"
[server]
listen = "127.0.0.1:{}"
ws_listen = "127.0.0.1:{}"

[upstream]
validators = [
{}
]

[rate_limit]
requests_per_second = 1000
burst = 2000

[metrics]
listen = "127.0.0.1:{}"
"#,
            self.config
                .infrastructure
                .rpc_gateway
                .rpc_port
                .unwrap_or(34310),
            self.config
                .infrastructure
                .rpc_gateway
                .metrics_port
                .unwrap_or(35311),
            self.config
                .validators
                .iter()
                .map(|v| format!("\"http://127.0.0.1:{}\"", v.rpc_port))
                .collect::<Vec<_>>()
                .join(",\n"),
            self.config
                .infrastructure
                .rpc_gateway
                .metrics_port
                .unwrap_or(35311)
        );
        std::fs::write(
            &self.config.infrastructure.rpc_gateway.config_path,
            gateway_config,
        )?;

        // Adversarial peer config
        let adversary_config = format!(
            r#"
[network]
p2p_port = {}
bootnodes = ["/ip4/127.0.0.1/tcp/{}"]
dual_kem = "off"

[rpc]
listen = "127.0.0.1:{}"
"#,
            self.config
                .infrastructure
                .adversarial_peer
                .p2p_port
                .unwrap_or(33320),
            self.config
                .infrastructure
                .bootnode
                .p2p_port
                .unwrap_or(33310),
            self.config
                .infrastructure
                .adversarial_peer
                .rpc_port
                .unwrap_or(34320)
        );
        std::fs::write(
            &self.config.infrastructure.adversarial_peer.config_path,
            adversary_config,
        )?;

        Ok(())
    }

    async fn write_monitoring_configs(&self) -> Result<()> {
        // Prometheus config
        let prometheus_config = format!(
            r#"
global:
  scrape_interval: {}s
  evaluation_interval: 15s

scrape_configs:
  - job_name: 'validators'
    static_configs:
      - targets: [{}]
        labels:
          group: 'validators'
  - job_name: 'infrastructure'
    static_configs:
      - targets: ["127.0.0.1:35310", "127.0.0.1:35311", "127.0.0.1:34310", "127.0.0.1:33310", "127.0.0.1:33320"]
        labels:
          group: 'infrastructure'
  - job_name: 'prometheus'
    static_configs:
      - targets: ['localhost:9090']
"#,
            self.config.monitoring.prometheus.scrape_interval_secs,
            self.config
                .monitoring
                .prometheus
                .scrape_targets
                .iter()
                .map(|t| format!("\"{}\"", t))
                .collect::<Vec<_>>()
                .join(", ")
        );
        std::fs::write(
            &self.config.monitoring.prometheus.config_path,
            prometheus_config,
        )?;

        // Alertmanager config
        let alertmanager_config = r#"
global:
  resolve_timeout: 5m

route:
  group_by: ['alertname']
  group_wait: 10s
  group_interval: 10s
  repeat_interval: 1h
  receiver: 'default'

receivers:
  - name: 'default'
    webhook_configs:
      - url: 'http://127.0.0.1:35314/alerts'
        send_resolved: true

inhibit_rules:
  - source_match:
      severity: 'critical'
    target_match:
      severity: 'warning'
    equal: ['alertname', 'instance']
"#;
        std::fs::write(
            &self.config.monitoring.alertmanager.config_path,
            alertmanager_config,
        )?;

        // Vector config
        let vector_config = r#"
sources:
  validator_logs:
    type: "file"
    include: ["D:\Maya2C-attacknet-v*/node.log"]
    read_from: "beginning"
    fingerprint:
      lines: 10

  infra_logs:
    type: "file"
    include: ["D:\Maya2C-attacknet-*/*.log"]
    read_from: "beginning"

transforms:
  parse_json:
    type: "remap"
    inputs: ["validator_logs", "infra_logs"]
    source: |
      . = parse_json!(.message)

sinks:
  prometheus:
    type: "prometheus_exporter"
    address: "127.0.0.1:35313"
    inputs: ["parse_json"]
"#;
        std::fs::write(&self.config.monitoring.vector.config_path, vector_config)?;

        Ok(())
    }

    /// Start all lab components
    pub async fn start_all(&self) -> Result<()> {
        info!("Starting attacknet lab...");

        // Start bootnode first
        self.start_bootnode().await?;

        // Wait for bootnode to be ready
        tokio::time::sleep(Duration::from_secs(2)).await;

        // Start validators
        self.validator_manager.start_all(None).await?;

        // Wait for consensus
        self.wait_for_consensus().await?;

        // Start RPC gateway
        self.start_rpc_gateway().await?;

        // Start adversarial peer
        self.start_adversarial_peer().await?;

        // Start monitoring stack (in WSL2)
        self.start_monitoring().await?;

        info!("Lab started successfully");
        Ok(())
    }

    async fn start_bootnode(&self) -> Result<()> {
        info!("Starting bootnode...");

        let bootnode = &self.config.infrastructure.bootnode;
        let binary = Self::find_binary(&bootnode.binary)?;

        let genesis_path = Path::new(&self.config.paths.genesis_dir).join("genesis.json");
        let port = |p: Option<u16>, what: &str| {
            p.ok_or_else(|| anyhow::anyhow!("bootnode {what} port is not configured"))
        };
        let rpc_port = port(bootnode.rpc_port, "rpc")?;
        let p2p_port = port(bootnode.p2p_port, "p2p")?;
        let metrics_port = port(bootnode.metrics_port, "metrics")?;
        let mut cmd = tokio::process::Command::new(binary);
        cmd.arg("--genesis")
            .arg(&genesis_path)
            .arg("--data-dir")
            .arg(&bootnode.data_dir)
            .arg("--rpc-addr")
            .arg(format!("127.0.0.1:{rpc_port}"))
            .arg("--p2p-port")
            .arg(p2p_port.to_string())
            .arg("--metrics-addr")
            .arg(format!("127.0.0.1:{metrics_port}"));

        let log_file = File::options()
            .create(true)
            .append(true)
            .open(&bootnode.log_path)?;

        cmd.stdout(log_file.try_clone()?)
            .stderr(log_file)
            .stdin(Stdio::null());

        let child = cmd.spawn()?;
        let mut processes = self.infrastructure_processes.lock().await;
        processes.insert("bootnode".to_string(), child);

        info!("Bootnode started");
        Ok(())
    }

    async fn start_rpc_gateway(&self) -> Result<()> {
        info!("Starting RPC gateway...");

        let gateway = &self.config.infrastructure.rpc_gateway;
        let binary = Self::find_binary(&gateway.binary)?;

        let mut cmd = tokio::process::Command::new(binary);
        cmd.arg("--config").arg(&gateway.config_path);

        let log_file = File::options()
            .create(true)
            .append(true)
            .open(&gateway.log_path)?;

        cmd.stdout(log_file.try_clone()?)
            .stderr(log_file)
            .stdin(Stdio::null());

        let child = cmd.spawn()?;
        let mut processes = self.infrastructure_processes.lock().await;
        processes.insert("rpc-gateway".to_string(), child);

        info!("RPC gateway started");
        Ok(())
    }

    async fn start_adversarial_peer(&self) -> Result<()> {
        info!("Starting adversarial peer...");

        let adversary = &self.config.infrastructure.adversarial_peer;

        // Check if adversary binary exists, if not, we'll skip for now
        let binary_path = PathBuf::from(&adversary.binary);
        if !binary_path.exists() && !Self::find_binary(&adversary.binary).is_ok() {
            warn!("Adversarial peer binary not found, skipping");
            return Ok(());
        }

        let binary = Self::find_binary(&adversary.binary).unwrap_or(binary_path);

        let mut cmd = tokio::process::Command::new(binary);
        cmd.arg("--config").arg(&adversary.config_path);

        let log_file = File::options()
            .create(true)
            .append(true)
            .open(&adversary.log_path)?;

        cmd.stdout(log_file.try_clone()?)
            .stderr(log_file)
            .stdin(Stdio::null());

        let child = cmd.spawn()?;
        let mut processes = self.infrastructure_processes.lock().await;
        processes.insert("adversarial-peer".to_string(), child);

        info!("Adversarial peer started");
        Ok(())
    }

    async fn start_monitoring(&self) -> Result<()> {
        info!("Starting monitoring stack (requires WSL2/Docker)...");

        // Check if Docker/WSL2 is available
        let output = tokio::process::Command::new("wsl")
            .args(["--", "docker", "version"])
            .output()
            .await;

        if !output.is_ok_and(|o| o.status.success()) {
            warn!("Docker not available in WSL2, skipping monitoring stack");
            return Ok(());
        }

        // Start monitoring stack via docker-compose
        let compose_file =
            PathBuf::from(&self.config.paths.control_dir).join("docker-compose.monitoring.yml");
        if compose_file.exists() {
            tokio::process::Command::new("wsl")
                .args([
                    "--",
                    "docker-compose",
                    "-f",
                    &compose_file.to_string_lossy(),
                    "up",
                    "-d",
                ])
                .output()
                .await?;
            info!("Monitoring stack started");
        } else {
            warn!(
                "Monitoring docker-compose file not found at {}",
                compose_file.display()
            );
        }

        Ok(())
    }

    async fn wait_for_consensus(&self) -> Result<()> {
        info!("Waiting for consensus...");

        let validators = self.config.validator_names();
        let timeout = Duration::from_secs(120);
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            let mut all_healthy = true;
            let mut heights = Vec::new();

            for name in &validators {
                let Some(v) = self.config.get_validator(name) else {
                    all_healthy = false;
                    continue;
                };

                if let Ok(resp) = reqwest::get(&format!("{}/health", v.health_url())).await
                    && resp.status().is_success()
                    && let Ok(tip_resp) =
                        reqwest::get(&format!("{}/get_tip_height", v.rpc_url())).await
                    && let Ok(json) = tip_resp.json::<serde_json::Value>().await
                    && let Some(h) = json.get("result").and_then(|r| r.as_u64())
                {
                    heights.push(h);
                } else {
                    all_healthy = false;
                }
            }

            if let Some(&min_height) = heights.iter().min().filter(|&&h| all_healthy && h >= 3) {
                info!("Consensus reached at height {}", min_height);
                return Ok(());
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
        }

        anyhow::bail!("Consensus not reached within timeout");
    }

    /// Stop all lab components cleanly
    pub async fn stop_all(&self) -> Result<()> {
        info!("Stopping attacknet lab...");

        // Stop validators
        self.validator_manager.stop_all(false).await?;

        // Stop infrastructure
        let mut processes = self.infrastructure_processes.lock().await;
        for (name, mut child) in processes.drain() {
            info!("Stopping {}", name);
            let _ = child.kill().await;
            let _ = child.wait().await;
        }

        info!("Lab stopped");
        Ok(())
    }

    /// Emergency termination
    pub async fn emergency_down(&self) -> Result<()> {
        warn!("Emergency termination of attacknet lab...");

        // Kill all validators
        self.validator_manager.stop_all(true).await?;

        // Kill all infrastructure
        let mut processes = self.infrastructure_processes.lock().await;
        for (name, mut child) in processes.drain() {
            warn!("Force killing {}", name);
            let _ = child.kill().await;
        }

        // Kill any remaining processes
        #[cfg(windows)]
        {
            let _ = tokio::process::Command::new("taskkill")
                .args(["/F", "/FI", "IMAGENAME eq maya2c-node*"])
                .output()
                .await;
            let _ = tokio::process::Command::new("taskkill")
                .args(["/F", "/FI", "IMAGENAME eq maya-api-gateway*"])
                .output()
                .await;
            let _ = tokio::process::Command::new("taskkill")
                .args(["/F", "/FI", "IMAGENAME eq attacknet-adversary*"])
                .output()
                .await;
        }

        warn!("Emergency termination complete");
        Ok(())
    }

    /// Get lab status
    pub async fn status(&self) -> Result<()> {
        println!("=== Attacknet Lab Status ===");
        println!("Lab ID: {}", self.config.lab_id);
        println!("Chain ID: {}", self.config.chain_id);
        println!();

        println!("Validators:");
        let statuses = self.validator_manager.all_status().await;
        for status in statuses {
            println!(
                "  {}: {} (PID: {:?}, Restarts: {})",
                status.name,
                if status.running { "RUNNING" } else { "STOPPED" },
                status.pid,
                status.restart_count
            );
        }

        println!("\nInfrastructure:");
        let mut processes = self.infrastructure_processes.lock().await;
        for (name, child) in processes.iter_mut() {
            let running = matches!(child.try_wait(), Ok(None));
            println!(
                "  {}: {}",
                name,
                if running { "RUNNING" } else { "STOPPED" }
            );
        }

        println!("\nResources:");
        self.show_resource_usage().await?;

        Ok(())
    }

    async fn show_resource_usage(&self) -> Result<()> {
        // Get process memory/CPU usage
        #[cfg(windows)]
        {
            let output = tokio::process::Command::new("powershell")
                .args(["-Command", "Get-Process | Where-Object {$_.ProcessName -like 'maya2c*' -or $_.ProcessName -like 'attacknet*'} | Select-Object ProcessName, Id, CPU, WS, PM | Format-Table -AutoSize"])
                .output()
                .await?;
            println!("{}", String::from_utf8_lossy(&output.stdout));
        }

        Ok(())
    }

    /// Show metrics
    pub async fn show_metrics(&self) -> Result<()> {
        self.metrics.print_summary().await;
        Ok(())
    }

    /// Generate daily report
    pub async fn generate_report(&self, date: Option<String>) -> Result<()> {
        let date = date.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());

        let runs_dir = PathBuf::from(&self.config.paths.runs_dir).join(&date);
        if !runs_dir.exists() {
            anyhow::bail!("No runs found for date {}", date);
        }

        let mut runs = Vec::new();
        for entry in std::fs::read_dir(runs_dir)? {
            let entry = entry?;
            let evidence_path = entry.path().join("evidence.json");
            if evidence_path.exists() {
                runs.push(crate::evidence::load_evidence(&evidence_path)?);
            }
        }

        let output =
            PathBuf::from(&self.config.paths.reports_dir).join(format!("{}-aggregate.md", date));
        crate::evidence::generate_aggregate_report(&runs, &output)?;

        info!("Aggregate report generated: {}", output.display());
        Ok(())
    }

    /// Clean lab artifacts
    pub async fn clean(&self, all: bool) -> Result<()> {
        info!("Cleaning lab artifacts (all={})...", all);

        // Clean logs
        for validator in &self.config.validators {
            let log_dir = Path::new(&validator.data_dir);
            if log_dir.exists() {
                for entry in std::fs::read_dir(log_dir)? {
                    let entry = entry?;
                    if entry.file_name().to_string_lossy().ends_with(".log") {
                        if all {
                            std::fs::remove_file(entry.path())?;
                        } else {
                            // Rotate: keep only last 10
                            // Simplified - just truncate
                            std::fs::write(entry.path(), "")?;
                        }
                    }
                }
            }
        }

        if all {
            // Remove data directories (but not keys/configs)
            for validator in &self.config.validators {
                let data_dir = Path::new(&validator.data_dir);
                if data_dir.exists() {
                    // Keep validator.key and config.toml
                    for entry in std::fs::read_dir(data_dir)? {
                        let entry = entry?;
                        let name = entry.file_name();
                        if name != "validator.key" && name != "config.toml" {
                            if entry.file_type()?.is_dir() {
                                std::fs::remove_dir_all(entry.path())?;
                            } else {
                                std::fs::remove_file(entry.path())?;
                            }
                        }
                    }
                }
            }
        }

        info!("Clean complete");
        Ok(())
    }
}
