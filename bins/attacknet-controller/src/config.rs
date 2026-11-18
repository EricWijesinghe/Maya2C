//! Lab configuration management

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Main lab configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabConfig {
    /// Identifier written into run reports so results from different labs are not mixed.
    pub lab_id: String,
    /// Chain identifier for the lab network. Must match the genesis file the validators load.
    pub chain_id: String,
    /// Description of the host machine the lab runs on.
    pub host: HostConfig,
    /// Validator nodes, one entry per validator process.
    pub validators: Vec<ValidatorConfig>,
    /// Bootnode, RPC gateway, adversary peer and optional explorer indexer.
    pub infrastructure: InfrastructureConfig,
    /// Named network segments that fault injection targets.
    pub network: NetworkConfig,
    /// Prometheus, Grafana, Alertmanager and Vector settings.
    pub monitoring: MonitoringConfig,
    /// Storage, log-retention and emergency limits. Declared here; see the field docs on each type.
    pub limits: ResourceLimits,
    /// Filesystem locations the lab reads and writes.
    pub paths: PathsConfig,
}

impl Default for LabConfig {
    fn default() -> Self {
        Self {
            lab_id: "maya2c-attacknet-lab-v1".to_string(),
            chain_id: "maya2c-attacknet-lab".to_string(),
            host: HostConfig::default(),
            validators: Self::default_validators(),
            infrastructure: InfrastructureConfig::default(),
            network: NetworkConfig::default(),
            monitoring: MonitoringConfig::default(),
            limits: ResourceLimits::default(),
            paths: PathsConfig::default(),
        }
    }
}

impl LabConfig {
    /// Default validator configurations
    fn default_validators() -> Vec<ValidatorConfig> {
        vec![
            ValidatorConfig {
                name: "validator-1".to_string(),
                index: 0,
                data_dir: "D:\\Maya2C-attacknet-v1".to_string(),
                p2p_port: 33300,
                rpc_port: 34300,
                metrics_port: 35300,
                cpu_affinity: vec![0, 1, 2],
                memory_limit_mb: 1536,
                commit_limit_mb: 3072,
                disk_quota_gb: 5,
                validator_key_path: "D:\\Maya2C-attacknet-v1\\validator.key".to_string(),
                genesis_path: "D:\\Maya2C-attacknet-genesis\\genesis.json".to_string(),
                config_path: "D:\\Maya2C-attacknet-v1\\config.toml".to_string(),
                log_path: "D:\\Maya2C-attacknet-v1\\node.log".to_string(),
                startup_order: 1,
                restart_policy: RestartPolicy::OnFailure { max_retries: 3 },
            },
            ValidatorConfig {
                name: "validator-2".to_string(),
                index: 1,
                data_dir: "D:\\Maya2C-attacknet-v2".to_string(),
                p2p_port: 33301,
                rpc_port: 34301,
                metrics_port: 35301,
                cpu_affinity: vec![3, 4, 5],
                memory_limit_mb: 1536,
                commit_limit_mb: 3072,
                disk_quota_gb: 5,
                validator_key_path: "D:\\Maya2C-attacknet-v2\\validator.key".to_string(),
                genesis_path: "D:\\Maya2C-attacknet-genesis\\genesis.json".to_string(),
                config_path: "D:\\Maya2C-attacknet-v2\\config.toml".to_string(),
                log_path: "D:\\Maya2C-attacknet-v2\\node.log".to_string(),
                startup_order: 2,
                restart_policy: RestartPolicy::OnFailure { max_retries: 3 },
            },
            ValidatorConfig {
                name: "validator-3".to_string(),
                index: 2,
                data_dir: "D:\\Maya2C-attacknet-v3".to_string(),
                p2p_port: 33302,
                rpc_port: 34302,
                metrics_port: 35302,
                cpu_affinity: vec![6, 7, 8],
                memory_limit_mb: 1536,
                commit_limit_mb: 3072,
                disk_quota_gb: 5,
                validator_key_path: "D:\\Maya2C-attacknet-v3\\validator.key".to_string(),
                genesis_path: "D:\\Maya2C-attacknet-genesis\\genesis.json".to_string(),
                config_path: "D:\\Maya2C-attacknet-v3\\config.toml".to_string(),
                log_path: "D:\\Maya2C-attacknet-v3\\node.log".to_string(),
                startup_order: 3,
                restart_policy: RestartPolicy::OnFailure { max_retries: 3 },
            },
            ValidatorConfig {
                name: "validator-4".to_string(),
                index: 3,
                data_dir: "D:\\Maya2C-attacknet-v4".to_string(),
                p2p_port: 33303,
                rpc_port: 34303,
                metrics_port: 35303,
                cpu_affinity: vec![9, 10, 11],
                memory_limit_mb: 1536,
                commit_limit_mb: 3072,
                disk_quota_gb: 5,
                validator_key_path: "D:\\Maya2C-attacknet-v4\\validator.key".to_string(),
                genesis_path: "D:\\Maya2C-attacknet-genesis\\genesis.json".to_string(),
                config_path: "D:\\Maya2C-attacknet-v4\\config.toml".to_string(),
                log_path: "D:\\Maya2C-attacknet-v4\\node.log".to_string(),
                startup_order: 4,
                restart_policy: RestartPolicy::OnFailure { max_retries: 3 },
            },
        ]
    }

    /// Load configuration from file or create default
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Reading config from {}", path.display()))?;
            let config: LabConfig = toml::from_str(&content)
                .with_context(|| format!("Parsing config from {}", path.display()))?;
            Ok(config)
        } else {
            let config = Self::default();
            config.save(path)?;
            Ok(config)
        }
    }

    /// Save configuration to file
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Creating config directory {}", parent.display()))?;
        }
        let content = toml::to_string_pretty(self).context("Serializing config to TOML")?;
        std::fs::write(path, content)
            .with_context(|| format!("Writing config to {}", path.display()))?;
        Ok(())
    }

    /// Get validator by name
    pub fn get_validator(&self, name: &str) -> Option<&ValidatorConfig> {
        self.validators.iter().find(|v| v.name == name)
    }

    /// Get validator by index
    #[allow(dead_code)]
    pub fn get_validator_by_index(&self, index: usize) -> Option<&ValidatorConfig> {
        self.validators.iter().find(|v| v.index == index)
    }

    /// Get all validator names
    pub fn validator_names(&self) -> Vec<String> {
        self.validators.iter().map(|v| v.name.clone()).collect()
    }
}

/// Host system configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostConfig {
    /// Machine name, recorded for the report only.
    pub hostname: String,
    /// Operating system string, recorded for the report only.
    pub os: String,
    /// Logical CPU count of the host.
    pub cpu_cores: usize,
    /// Installed physical memory, in GiB.
    pub total_memory_gb: usize,
    /// Free space on drive D:, in GiB, at the time the config was written.
    pub disk_d_free_gb: usize,
    /// Logical cores kept back from the lab so the host stays responsive.
    pub reserved_cores: usize,
    /// Physical memory kept back from the lab, in GiB.
    pub reserved_memory_gb: usize,
    /// Commit charge kept back from the lab, in GiB.
    pub reserved_commit_gb: usize,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            hostname: "NITROZEUS".to_string(),
            os: "Windows 11 Pro 10.0.29680".to_string(),
            cpu_cores: 24,
            total_memory_gb: 32,
            disk_d_free_gb: 259,
            reserved_cores: 6,
            reserved_memory_gb: 4,
            reserved_commit_gb: 8,
        }
    }
}

/// Validator configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidatorConfig {
    /// Unique name used in logs, reports and the network segment endpoint lists.
    pub name: String,
    /// Zero-based position in the validator list; used to pick the node's slot.
    pub index: usize,
    /// Working directory for the node's state.
    pub data_dir: String,
    /// libp2p TCP port the node listens on, on 127.0.0.1.
    pub p2p_port: u16,
    /// JSON-RPC port; `rpc_url` and `health_url` are built from it.
    pub rpc_port: u16,
    /// Prometheus metrics port scraped by the monitoring stack.
    pub metrics_port: u16,
    /// Logical CPU indices the process is meant to run on. Stored only; this crate does not apply it.
    pub cpu_affinity: Vec<usize>,
    /// Memory ceiling for the process, in MiB. Stored only; this crate does not enforce it.
    pub memory_limit_mb: usize,
    /// Commit-charge ceiling for the process, in MiB. Stored only; this crate does not enforce it.
    pub commit_limit_mb: usize,
    /// Disk quota for `data_dir`, in GiB. Stored only; this crate does not enforce it.
    pub disk_quota_gb: usize,
    /// Path to the validator's signing key file. Read by genesis generation in `lab.rs` and passed to the node at start.
    pub validator_key_path: String,
    /// Path to the genesis file the node starts from.
    pub genesis_path: String,
    /// Path to the node's TOML configuration file.
    pub config_path: String,
    /// File the node's stdout and stderr are appended to.
    pub log_path: String,
    /// Start order; lower values start first. Validators are sorted by this before launch.
    pub startup_order: u8,
    /// Limit on explicit restart requests; see `RestartPolicy`.
    pub restart_policy: RestartPolicy,
}

impl ValidatorConfig {
    /// JSON-RPC base URL on localhost.
    pub fn rpc_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.rpc_port)
    }

    /// libp2p multiaddr for the local p2p port (TCP over IPv4 loopback).
    pub fn p2p_multiaddr(&self) -> String {
        format!("/ip4/127.0.0.1/tcp/{}", self.p2p_port)
    }

    /// Prometheus metrics URL on localhost. Unused in this crate today, so it is allowed to be dead code.
    #[allow(dead_code)]
    pub fn metrics_url(&self) -> String {
        format!("http://127.0.0.1:{}/metrics", self.metrics_port)
    }

    /// Health-check URL: `/health` on the RPC port, on localhost.
    pub fn health_url(&self) -> String {
        format!("http://127.0.0.1:{}/health", self.rpc_port)
    }
}

/// Restart policy for validators.
///
/// Checked by `ValidatorManager::restart` (`validators.rs`) on each restart
/// request, before the process is stopped and started again.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RestartPolicy {
    /// Refuse every restart request.
    Never,
    /// Allow restarts without a limit.
    Always,
    /// Refuse a restart once the validator has been restarted more than `max_retries` times.
    OnFailure {
        /// Number of restarts allowed before the policy refuses.
        max_retries: u32,
    },
}

/// Infrastructure services configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfrastructureConfig {
    /// Node that other validators dial to join the lab network.
    pub bootnode: ServiceConfig,
    /// API gateway that exposes the validators' RPC to clients.
    pub rpc_gateway: ServiceConfig,
    /// Node that sends hostile traffic during attack scenarios.
    pub adversarial_peer: ServiceConfig,
    /// Optional block explorer indexer; `None` means it is not run.
    pub explorer_indexer: Option<ServiceConfig>,
}

impl Default for InfrastructureConfig {
    fn default() -> Self {
        Self {
            bootnode: ServiceConfig {
                name: "bootnode".to_string(),
                binary: "maya2c-node".to_string(),
                data_dir: "D:\\Maya2C-attacknet-bootnode".to_string(),
                config_path: "D:\\Maya2C-attacknet-bootnode\\config.toml".to_string(),
                genesis_path: Some("D:\\Maya2C-attacknet-genesis\\genesis.json".to_string()),
                p2p_port: Some(33310),
                rpc_port: Some(34310),
                metrics_port: Some(35310),
                cpu_affinity: vec![12],
                memory_limit_mb: 256,
                commit_limit_mb: 512,
                disk_quota_gb: 1,
                log_path: "D:\\Maya2C-attacknet-bootnode\\node.log".to_string(),
                startup_order: 0,
                restart_policy: RestartPolicy::Always,
                extra_args: vec!["--bootnode".to_string()],
            },
            rpc_gateway: ServiceConfig {
                name: "rpc-gateway".to_string(),
                binary: "maya-api-gateway".to_string(),
                data_dir: "D:\\Maya2C-attacknet-gateway".to_string(),
                config_path: "D:\\Maya2C-attacknet-gateway\\config.toml".to_string(),
                genesis_path: None,
                p2p_port: None,
                rpc_port: Some(34310),
                metrics_port: Some(35311),
                cpu_affinity: vec![13, 14],
                memory_limit_mb: 512,
                commit_limit_mb: 1024,
                disk_quota_gb: 2,
                log_path: "D:\\Maya2C-attacknet-gateway\\gateway.log".to_string(),
                startup_order: 5,
                restart_policy: RestartPolicy::OnFailure { max_retries: 3 },
                extra_args: vec![],
            },
            adversarial_peer: ServiceConfig {
                name: "adversarial-peer".to_string(),
                binary: "attacknet-adversary".to_string(),
                data_dir: "D:\\Maya2C-attacknet-adversary".to_string(),
                config_path: "D:\\Maya2C-attacknet-adversary\\config.toml".to_string(),
                genesis_path: Some("D:\\Maya2C-attacknet-genesis\\genesis.json".to_string()),
                p2p_port: Some(33320),
                rpc_port: Some(34320),
                metrics_port: None,
                cpu_affinity: vec![15],
                memory_limit_mb: 512,
                commit_limit_mb: 1024,
                disk_quota_gb: 1,
                log_path: "D:\\Maya2C-attacknet-adversary\\adversary.log".to_string(),
                startup_order: 6,
                restart_policy: RestartPolicy::OnFailure { max_retries: 1 },
                extra_args: vec![],
            },
            explorer_indexer: None,
        }
    }
}

/// Generic service configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// Unique name used in logs and reports.
    pub name: String,
    /// Executable to run, looked up on PATH or given as a path.
    pub binary: String,
    /// Working directory for the service's state.
    pub data_dir: String,
    /// Path to the service's configuration file.
    pub config_path: String,
    /// Genesis file to load, or `None` when the service does not read one.
    pub genesis_path: Option<String>,
    /// libp2p port, or `None` when the service does not listen on p2p.
    pub p2p_port: Option<u16>,
    /// RPC port, or `None` when the service serves no RPC.
    pub rpc_port: Option<u16>,
    /// Metrics port, or `None` when the service exports no metrics.
    pub metrics_port: Option<u16>,
    /// Logical CPU indices the service is meant to run on. Stored only; this crate does not apply it.
    pub cpu_affinity: Vec<usize>,
    /// Memory ceiling for the service, in MiB. Stored only; this crate does not enforce it.
    pub memory_limit_mb: usize,
    /// Commit-charge ceiling for the service, in MiB. Stored only; this crate does not enforce it.
    pub commit_limit_mb: usize,
    /// Disk quota for `data_dir`, in GiB. Stored only; this crate does not enforce it.
    pub disk_quota_gb: usize,
    /// File the service's stdout and stderr are appended to.
    pub log_path: String,
    /// Declared start order. No code in this crate applies it to services; only validators are sorted by it.
    pub startup_order: u8,
    /// Declared restart policy. No code in this crate applies it to services; see `validators.rs` for the validator behaviour.
    pub restart_policy: RestartPolicy,
    /// Extra command-line arguments appended after the generated ones.
    pub extra_args: Vec<String>,
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Segments keyed by id (`S1`..`S8` in the defaults).
    pub segments: HashMap<String, NetworkSegment>,
    /// Which fault tools are used per layer, and what each tool can do.
    pub fault_injection: FaultInjectionConfig,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        let mut segments = HashMap::new();
        segments.insert(
            "S1".to_string(),
            NetworkSegment {
                id: "S1".to_string(),
                name: "validator_mesh".to_string(),
                endpoints: vec![
                    "validator-1".to_string(),
                    "validator-2".to_string(),
                    "validator-3".to_string(),
                    "validator-4".to_string(),
                ],
                protocol: "libp2p".to_string(),
                direction: "bidirectional".to_string(),
            },
        );
        segments.insert(
            "S2".to_string(),
            NetworkSegment {
                id: "S2".to_string(),
                name: "validator_to_bootnode".to_string(),
                endpoints: vec![
                    "validator-1".to_string(),
                    "validator-2".to_string(),
                    "validator-3".to_string(),
                    "validator-4".to_string(),
                    "bootnode".to_string(),
                ],
                protocol: "libp2p".to_string(),
                direction: "outbound".to_string(),
            },
        );
        segments.insert(
            "S3".to_string(),
            NetworkSegment {
                id: "S3".to_string(),
                name: "bootnode_to_validator".to_string(),
                endpoints: vec![
                    "bootnode".to_string(),
                    "validator-1".to_string(),
                    "validator-2".to_string(),
                    "validator-3".to_string(),
                    "validator-4".to_string(),
                ],
                protocol: "libp2p".to_string(),
                direction: "inbound".to_string(),
            },
        );
        segments.insert(
            "S4".to_string(),
            NetworkSegment {
                id: "S4".to_string(),
                name: "rpc_client_to_validator".to_string(),
                endpoints: vec![
                    "*".to_string(),
                    "validator-1".to_string(),
                    "validator-2".to_string(),
                    "validator-3".to_string(),
                    "validator-4".to_string(),
                ],
                protocol: "jsonrpc".to_string(),
                direction: "inbound".to_string(),
            },
        );
        segments.insert(
            "S5".to_string(),
            NetworkSegment {
                id: "S5".to_string(),
                name: "gateway_to_validator".to_string(),
                endpoints: vec![
                    "rpc-gateway".to_string(),
                    "validator-1".to_string(),
                    "validator-2".to_string(),
                    "validator-3".to_string(),
                    "validator-4".to_string(),
                ],
                protocol: "jsonrpc".to_string(),
                direction: "outbound".to_string(),
            },
        );
        segments.insert(
            "S6".to_string(),
            NetworkSegment {
                id: "S6".to_string(),
                name: "adversary_to_validator".to_string(),
                endpoints: vec![
                    "adversarial-peer".to_string(),
                    "validator-1".to_string(),
                    "validator-2".to_string(),
                    "validator-3".to_string(),
                    "validator-4".to_string(),
                ],
                protocol: "libp2p".to_string(),
                direction: "outbound".to_string(),
            },
        );
        segments.insert(
            "S7".to_string(),
            NetworkSegment {
                id: "S7".to_string(),
                name: "validator_to_adversary".to_string(),
                endpoints: vec![
                    "validator-1".to_string(),
                    "validator-2".to_string(),
                    "validator-3".to_string(),
                    "validator-4".to_string(),
                    "adversarial-peer".to_string(),
                ],
                protocol: "libp2p".to_string(),
                direction: "inbound".to_string(),
            },
        );
        segments.insert(
            "S8".to_string(),
            NetworkSegment {
                id: "S8".to_string(),
                name: "metrics_scraping".to_string(),
                endpoints: vec![
                    "monitoring-prometheus".to_string(),
                    "validator-1".to_string(),
                    "validator-2".to_string(),
                    "validator-3".to_string(),
                    "validator-4".to_string(),
                    "rpc-gateway".to_string(),
                    "bootnode".to_string(),
                    "adversarial-peer".to_string(),
                ],
                protocol: "prometheus".to_string(),
                direction: "outbound".to_string(),
            },
        );

        Self {
            segments,
            fault_injection: FaultInjectionConfig::default(),
        }
    }
}

/// Network segment definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSegment {
    /// Key this segment is stored under in `NetworkConfig::segments`.
    pub id: String,
    /// Human-readable label for logs and reports.
    pub name: String,
    /// Node or service names in this segment. Stored only; no code in this crate reads it.
    pub endpoints: Vec<String>,
    /// Application protocol carried on the segment (`libp2p`, `jsonrpc` or `prometheus`).
    pub protocol: String,
    /// Traffic direction relative to the endpoints: `inbound`, `outbound` or `bidirectional`.
    pub direction: String,
}

/// Fault injection configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FaultInjectionConfig {
    /// Process-level faults (kill, restart, affinity, memory limit).
    pub process: ProcessFaultConfig,
    /// Packet-level faults (drop, delay, reset) on the wire.
    pub network_l4: NetworkL4FaultConfig,
    /// Proxy-level faults (latency, bandwidth, loss) on TCP streams.
    pub network_l7: NetworkL7FaultConfig,
    /// Protocol-level faults (malformed frames, bad handshakes) from the adversary peer.
    pub network_protocol: NetworkProtocolFaultConfig,
    /// Disk and filesystem faults (quota, truncation, read-only, corruption).
    pub storage: StorageFaultConfig,
}

/// Process-level fault injection: the tool used and the operations it supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessFaultConfig {
    /// Name of the tool that performs the fault.
    pub tool: String,
    /// Operations the tool is declared to support. Stored only; no code in this crate reads it.
    pub capabilities: Vec<String>,
}

impl Default for ProcessFaultConfig {
    fn default() -> Self {
        Self {
            tool: "powershell_job_objects".to_string(),
            capabilities: vec![
                "kill".to_string(),
                "start".to_string(),
                "restart".to_string(),
                "affinity".to_string(),
                "memory_limit".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Packet-level (L4) fault injection settings.
pub struct NetworkL4FaultConfig {
    /// Name of the tool that performs the fault.
    pub tool: String,
    /// Operations the tool is declared to support. Stored only; no code in this crate reads it.
    pub capabilities: Vec<String>,
}

impl Default for NetworkL4FaultConfig {
    fn default() -> Self {
        Self {
            tool: "pktmon".to_string(),
            capabilities: vec![
                "drop".to_string(),
                "corrupt".to_string(),
                "delay".to_string(),
                "reset".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Proxy-level (L7) fault injection settings.
pub struct NetworkL7FaultConfig {
    /// Name of the tool that performs the fault.
    pub tool: String,
    /// Operations the tool is declared to support. Stored only; no code in this crate reads it.
    pub capabilities: Vec<String>,
}

impl Default for NetworkL7FaultConfig {
    fn default() -> Self {
        Self {
            tool: "toxiproxy_windows".to_string(),
            capabilities: vec![
                "latency".to_string(),
                "bandwidth".to_string(),
                "loss".to_string(),
                "jitter".to_string(),
                "reorder".to_string(),
                "duplicate".to_string(),
                "reset".to_string(),
                "close".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Protocol-level fault injection settings, delivered by the adversary peer.
pub struct NetworkProtocolFaultConfig {
    /// Name of the tool that performs the fault.
    pub tool: String,
    /// Operations the tool is declared to support. Stored only; no code in this crate reads it.
    pub capabilities: Vec<String>,
}

impl Default for NetworkProtocolFaultConfig {
    fn default() -> Self {
        Self {
            tool: "custom_rust_proxy".to_string(),
            capabilities: vec![
                "malformed_frames".to_string(),
                "invalid_handshakes".to_string(),
                "oversized".to_string(),
                "slow_send".to_string(),
                "churn".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Storage fault injection settings.
pub struct StorageFaultConfig {
    /// Name of the tool that performs the fault.
    pub tool: String,
    /// Operations the tool is declared to support. Stored only; no code in this crate reads it.
    pub capabilities: Vec<String>,
}

impl Default for StorageFaultConfig {
    fn default() -> Self {
        Self {
            tool: "ntfs_quota_powershell".to_string(),
            capabilities: vec![
                "quota_enforce".to_string(),
                "file_replace".to_string(),
                "truncate".to_string(),
                "readonly".to_string(),
                "corrupt".to_string(),
            ],
        }
    }
}

/// Monitoring configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MonitoringConfig {
    /// Metrics collection and storage.
    pub prometheus: PrometheusConfig,
    /// Dashboards over the Prometheus data.
    pub grafana: GrafanaConfig,
    /// Alert routing.
    pub alertmanager: AlertmanagerConfig,
    /// Log shipping.
    pub vector: VectorConfig,
}

/// Prometheus settings for the lab's metrics stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrometheusConfig {
    /// Container image to run.
    pub image: String,
    /// HTTP port Prometheus listens on.
    pub port: u16,
    /// Storage directory, created by the controller before the stack starts.
    pub data_dir: String,
    /// Path to the generated Prometheus scrape configuration.
    pub config_path: String,
    /// How long TSDB data is kept, in days.
    pub retention_days: u32,
    /// Scrape interval, in seconds.
    pub scrape_interval_secs: u32,
    /// `host:port` endpoints scraped for metrics, one per exporter.
    pub scrape_targets: Vec<String>,
}

impl Default for PrometheusConfig {
    fn default() -> Self {
        Self {
            image: "prom/prometheus:v2.54.0".to_string(),
            port: 35310,
            data_dir: "/mnt/d/Maya2C-attacknet-monitoring/prometheus".to_string(),
            config_path: "/mnt/d/Maya2C-attacknet-monitoring/prometheus.yml".to_string(),
            retention_days: 14,
            scrape_interval_secs: 15,
            scrape_targets: vec![
                "127.0.0.1:35300".to_string(),
                "127.0.0.1:35301".to_string(),
                "127.0.0.1:35302".to_string(),
                "127.0.0.1:35303".to_string(),
                "127.0.0.1:35310".to_string(),
                "127.0.0.1:35311".to_string(),
                "127.0.0.1:34310".to_string(),
                "127.0.0.1:33310".to_string(),
                "127.0.0.1:33320".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Grafana settings for the lab's dashboards.
pub struct GrafanaConfig {
    /// Container image to run.
    pub image: String,
    /// HTTP port Grafana listens on.
    pub port: u16,
    /// Storage directory for Grafana's database and plugins.
    pub data_dir: String,
    /// Login name of the built-in admin account.
    pub admin_user: String,
    /// Password of the built-in admin account. Lab default, stored as plain text in this struct.
    pub admin_password: String,
    /// Directory of dashboard JSON files provisioned into Grafana.
    pub dashboards_dir: String,
}

impl Default for GrafanaConfig {
    fn default() -> Self {
        Self {
            image: "grafana/grafana:11.1.0".to_string(),
            port: 35311,
            data_dir: "/mnt/d/Maya2C-attacknet-monitoring/grafana".to_string(),
            admin_user: "admin".to_string(),
            admin_password: "lab".to_string(),
            dashboards_dir: "/mnt/d/Maya2C-attacknet-monitoring/dashboards".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Alertmanager settings for routing alerts from Prometheus.
pub struct AlertmanagerConfig {
    /// Container image to run.
    pub image: String,
    /// HTTP port Alertmanager listens on.
    pub port: u16,
    /// Storage directory for silences and notification state.
    pub data_dir: String,
    /// Path to the Alertmanager configuration file.
    pub config_path: String,
}

impl Default for AlertmanagerConfig {
    fn default() -> Self {
        Self {
            image: "prom/alertmanager:v0.27.0".to_string(),
            port: 35312,
            data_dir: "/mnt/d/Maya2C-attacknet-monitoring/alertmanager".to_string(),
            config_path: "/mnt/d/Maya2C-attacknet-monitoring/alertmanager.yml".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Vector settings for shipping node and service logs.
pub struct VectorConfig {
    /// Container image to run.
    pub image: String,
    /// Port Vector listens on.
    pub port: u16,
    /// Storage directory for Vector's buffers and checkpoints.
    pub data_dir: String,
    /// Path to the Vector pipeline configuration file.
    pub config_path: String,
}

impl Default for VectorConfig {
    fn default() -> Self {
        Self {
            image: "timberio/vector:0.42.0-alpine".to_string(),
            port: 35313,
            data_dir: "/mnt/d/Maya2C-attacknet-monitoring/vector".to_string(),
            config_path: "/mnt/d/Maya2C-attacknet-monitoring/vector.yml".to_string(),
        }
    }
}

/// Resource limits
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResourceLimits {
    /// Per-directory size budgets.
    pub storage: StorageLimits,
    /// How long each kind of log is kept.
    pub log_retention: LogRetention,
    /// Host and validator levels that trigger an emergency stop.
    pub emergency_thresholds: EmergencyThresholds,
}

/// Size budgets per directory. Each pair is a soft limit (warn) and a hard limit (act).
///
/// Declared in config only. No code in this crate reads these fields yet, so the
/// budgets are not enforced by this crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageLimits {
    /// Soft size budget for one validator's data directory, in GiB.
    pub validator_data_soft_gb: usize,
    /// Hard size budget for one validator's data directory, in GiB.
    pub validator_data_hard_gb: usize,
    /// Soft size budget for one validator's log file, in MiB.
    pub validator_logs_soft_mb: usize,
    /// Hard size budget for one validator's log file, in MiB.
    pub validator_logs_hard_mb: usize,
    /// Soft size budget for the reports directory, in GiB.
    pub reports_soft_gb: usize,
    /// Hard size budget for the reports directory, in GiB.
    pub reports_hard_gb: usize,
    /// Soft size budget for the monitoring data, in GiB.
    pub monitoring_soft_gb: usize,
    /// Hard size budget for the monitoring data, in GiB.
    pub monitoring_hard_gb: usize,
}

impl Default for StorageLimits {
    fn default() -> Self {
        Self {
            validator_data_soft_gb: 4,
            validator_data_hard_gb: 5,
            validator_logs_soft_mb: 400,
            validator_logs_hard_mb: 500,
            reports_soft_gb: 1,
            reports_hard_gb: 2,
            monitoring_soft_gb: 4,
            monitoring_hard_gb: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Retention periods per log kind. Declared in config only; no code in this crate reads them yet.
pub struct LogRetention {
    /// Days to keep validator stdout logs.
    pub validator_stdout_days: u32,
    /// Days to keep consensus trace logs.
    pub consensus_traces_days: u32,
    /// Days to keep RPC access logs.
    pub rpc_access_days: u32,
    /// Days to keep attack artifacts (evidence bundles).
    pub attack_artifacts_days: u32,
    /// Days to keep metrics data.
    pub metrics_days: u32,
    /// Hours after which each log kind is compressed, keyed by kind name
    /// (`validator_stdout`, `consensus_traces`, `rpc_access`, `attack_artifacts`).
    pub compress_after_hours: HashMap<String, u32>,
}

impl Default for LogRetention {
    fn default() -> Self {
        let mut compress = HashMap::new();
        compress.insert("validator_stdout".to_string(), 24);
        compress.insert("consensus_traces".to_string(), 6);
        compress.insert("rpc_access".to_string(), 1);
        compress.insert("attack_artifacts".to_string(), 0);
        Self {
            validator_stdout_days: 7,
            consensus_traces_days: 3,
            rpc_access_days: 1,
            attack_artifacts_days: 30,
            metrics_days: 14,
            compress_after_hours: compress,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Resource thresholds for the emergency-stop logic.
///
/// Declared in config only. No code in this crate reads these fields yet, so
/// no emergency stop is triggered by this crate.
pub struct EmergencyThresholds {
    /// Host commit-charge threshold, in GiB.
    pub host_commit_charge_gb: usize,
    /// Host free physical RAM threshold, in GiB.
    pub host_physical_ram_free_gb: usize,
    /// Free space threshold on drive D:, in GiB.
    pub disk_d_free_gb: usize,
    /// Per-validator CPU threshold, in percent.
    pub validator_cpu_percent: u8,
    /// Per-validator memory threshold, in MiB.
    pub validator_ram_mb: usize,
    /// Per-validator disk threshold, in GiB.
    pub validator_disk_gb: usize,
    /// Threshold for how far log collection may lag, in minutes.
    pub log_collection_lag_minutes: usize,
    /// Longest a single scenario may run, in hours.
    pub scenario_max_hours: usize,
}

impl Default for EmergencyThresholds {
    fn default() -> Self {
        Self {
            host_commit_charge_gb: 42,
            host_physical_ram_free_gb: 1,
            disk_d_free_gb: 20,
            validator_cpu_percent: 90,
            validator_ram_mb: 1800,
            validator_disk_gb: 4,
            log_collection_lag_minutes: 10,
            scenario_max_hours: 4,
        }
    }
}

/// Paths configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    /// Root of the lab's working area.
    pub base_dir: String,
    /// Directory holding `genesis.json`, which the controller writes and the nodes read.
    pub genesis_dir: String,
    /// Directory for the reports written after each run.
    pub reports_dir: String,
    /// Directory for per-run output.
    pub runs_dir: String,
    /// Directory for control files shared with the lab's compose setup.
    pub control_dir: String,
    /// Directory of helper tools (`docker-compose` and similar).
    pub tools_dir: String,
}

impl Default for PathsConfig {
    fn default() -> Self {
        Self {
            base_dir: "D:\\Maya2C-attacknet".to_string(),
            genesis_dir: "D:\\Maya2C-attacknet-genesis".to_string(),
            reports_dir: "D:\\Maya2C\\reports\\attacknet".to_string(),
            runs_dir: "D:\\Maya2C\\reports\\attacknet\\runs".to_string(),
            control_dir: "D:\\Maya2C-attacknet-control".to_string(),
            tools_dir: "D:\\Maya2C-tools".to_string(),
        }
    }
}
