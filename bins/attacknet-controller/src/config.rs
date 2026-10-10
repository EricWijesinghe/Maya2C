//! Lab configuration management

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Main lab configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabConfig {
    pub lab_id: String,
    pub chain_id: String,
    pub host: HostConfig,
    pub validators: Vec<ValidatorConfig>,
    pub infrastructure: InfrastructureConfig,
    pub network: NetworkConfig,
    pub monitoring: MonitoringConfig,
    pub limits: ResourceLimits,
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
        let content = toml::to_string_pretty(self)
            .context("Serializing config to TOML")?;
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
    pub hostname: String,
    pub os: String,
    pub cpu_cores: usize,
    pub total_memory_gb: usize,
    pub disk_d_free_gb: usize,
    pub reserved_cores: usize,
    pub reserved_memory_gb: usize,
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
    pub name: String,
    pub index: usize,
    pub data_dir: String,
    pub p2p_port: u16,
    pub rpc_port: u16,
    pub metrics_port: u16,
    pub cpu_affinity: Vec<usize>,
    pub memory_limit_mb: usize,
    pub commit_limit_mb: usize,
    pub disk_quota_gb: usize,
    pub validator_key_path: String,
    pub genesis_path: String,
    pub config_path: String,
    pub log_path: String,
    pub startup_order: u8,
    pub restart_policy: RestartPolicy,
}

impl ValidatorConfig {
    pub fn rpc_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.rpc_port)
    }

    pub fn p2p_multiaddr(&self) -> String {
        format!("/ip4/127.0.0.1/tcp/{}", self.p2p_port)
    }

    #[allow(dead_code)]
    pub fn metrics_url(&self) -> String {
        format!("http://127.0.0.1:{}/metrics", self.metrics_port)
    }

    pub fn health_url(&self) -> String {
        format!("http://127.0.0.1:{}/health", self.rpc_port)
    }
}

/// Restart policy for validators
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RestartPolicy {
    Never,
    Always,
    OnFailure { max_retries: u32 },
}

/// Infrastructure services configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfrastructureConfig {
    pub bootnode: ServiceConfig,
    pub rpc_gateway: ServiceConfig,
    pub adversarial_peer: ServiceConfig,
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
    pub name: String,
    pub binary: String,
    pub data_dir: String,
    pub config_path: String,
    pub genesis_path: Option<String>,
    pub p2p_port: Option<u16>,
    pub rpc_port: Option<u16>,
    pub metrics_port: Option<u16>,
    pub cpu_affinity: Vec<usize>,
    pub memory_limit_mb: usize,
    pub commit_limit_mb: usize,
    pub disk_quota_gb: usize,
    pub log_path: String,
    pub startup_order: u8,
    pub restart_policy: RestartPolicy,
    pub extra_args: Vec<String>,
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub segments: HashMap<String, NetworkSegment>,
    pub fault_injection: FaultInjectionConfig,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        let mut segments = HashMap::new();
        segments.insert("S1".to_string(), NetworkSegment {
            id: "S1".to_string(),
            name: "validator_mesh".to_string(),
            endpoints: vec!["validator-1".to_string(), "validator-2".to_string(), "validator-3".to_string(), "validator-4".to_string()],
            protocol: "libp2p".to_string(),
            direction: "bidirectional".to_string(),
        });
        segments.insert("S2".to_string(), NetworkSegment {
            id: "S2".to_string(),
            name: "validator_to_bootnode".to_string(),
            endpoints: vec!["validator-1".to_string(), "validator-2".to_string(), "validator-3".to_string(), "validator-4".to_string(), "bootnode".to_string()],
            protocol: "libp2p".to_string(),
            direction: "outbound".to_string(),
        });
        segments.insert("S3".to_string(), NetworkSegment {
            id: "S3".to_string(),
            name: "bootnode_to_validator".to_string(),
            endpoints: vec!["bootnode".to_string(), "validator-1".to_string(), "validator-2".to_string(), "validator-3".to_string(), "validator-4".to_string()],
            protocol: "libp2p".to_string(),
            direction: "inbound".to_string(),
        });
        segments.insert("S4".to_string(), NetworkSegment {
            id: "S4".to_string(),
            name: "rpc_client_to_validator".to_string(),
            endpoints: vec!["*".to_string(), "validator-1".to_string(), "validator-2".to_string(), "validator-3".to_string(), "validator-4".to_string()],
            protocol: "jsonrpc".to_string(),
            direction: "inbound".to_string(),
        });
        segments.insert("S5".to_string(), NetworkSegment {
            id: "S5".to_string(),
            name: "gateway_to_validator".to_string(),
            endpoints: vec!["rpc-gateway".to_string(), "validator-1".to_string(), "validator-2".to_string(), "validator-3".to_string(), "validator-4".to_string()],
            protocol: "jsonrpc".to_string(),
            direction: "outbound".to_string(),
        });
        segments.insert("S6".to_string(), NetworkSegment {
            id: "S6".to_string(),
            name: "adversary_to_validator".to_string(),
            endpoints: vec!["adversarial-peer".to_string(), "validator-1".to_string(), "validator-2".to_string(), "validator-3".to_string(), "validator-4".to_string()],
            protocol: "libp2p".to_string(),
            direction: "outbound".to_string(),
        });
        segments.insert("S7".to_string(), NetworkSegment {
            id: "S7".to_string(),
            name: "validator_to_adversary".to_string(),
            endpoints: vec!["validator-1".to_string(), "validator-2".to_string(), "validator-3".to_string(), "validator-4".to_string(), "adversarial-peer".to_string()],
            protocol: "libp2p".to_string(),
            direction: "inbound".to_string(),
        });
        segments.insert("S8".to_string(), NetworkSegment {
            id: "S8".to_string(),
            name: "metrics_scraping".to_string(),
            endpoints: vec!["monitoring-prometheus".to_string(), "validator-1".to_string(), "validator-2".to_string(), "validator-3".to_string(), "validator-4".to_string(), "rpc-gateway".to_string(), "bootnode".to_string(), "adversarial-peer".to_string()],
            protocol: "prometheus".to_string(),
            direction: "outbound".to_string(),
        });

        Self {
            segments,
            fault_injection: FaultInjectionConfig::default(),
        }
    }
}

/// Network segment definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkSegment {
    pub id: String,
    pub name: String,
    pub endpoints: Vec<String>,
    pub protocol: String,
    pub direction: String,
}

/// Fault injection configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FaultInjectionConfig {
    pub process: ProcessFaultConfig,
    pub network_l4: NetworkL4FaultConfig,
    pub network_l7: NetworkL7FaultConfig,
    pub network_protocol: NetworkProtocolFaultConfig,
    pub storage: StorageFaultConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessFaultConfig {
    pub tool: String,
    pub capabilities: Vec<String>,
}

impl Default for ProcessFaultConfig {
    fn default() -> Self {
        Self {
            tool: "powershell_job_objects".to_string(),
            capabilities: vec!["kill".to_string(), "start".to_string(), "restart".to_string(), "affinity".to_string(), "memory_limit".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkL4FaultConfig {
    pub tool: String,
    pub capabilities: Vec<String>,
}

impl Default for NetworkL4FaultConfig {
    fn default() -> Self {
        Self {
            tool: "pktmon".to_string(),
            capabilities: vec!["drop".to_string(), "corrupt".to_string(), "delay".to_string(), "reset".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkL7FaultConfig {
    pub tool: String,
    pub capabilities: Vec<String>,
}

impl Default for NetworkL7FaultConfig {
    fn default() -> Self {
        Self {
            tool: "toxiproxy_windows".to_string(),
            capabilities: vec!["latency".to_string(), "bandwidth".to_string(), "loss".to_string(), "jitter".to_string(), "reorder".to_string(), "duplicate".to_string(), "reset".to_string(), "close".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkProtocolFaultConfig {
    pub tool: String,
    pub capabilities: Vec<String>,
}

impl Default for NetworkProtocolFaultConfig {
    fn default() -> Self {
        Self {
            tool: "custom_rust_proxy".to_string(),
            capabilities: vec!["malformed_frames".to_string(), "invalid_handshakes".to_string(), "oversized".to_string(), "slow_send".to_string(), "churn".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageFaultConfig {
    pub tool: String,
    pub capabilities: Vec<String>,
}

impl Default for StorageFaultConfig {
    fn default() -> Self {
        Self {
            tool: "ntfs_quota_powershell".to_string(),
            capabilities: vec!["quota_enforce".to_string(), "file_replace".to_string(), "truncate".to_string(), "readonly".to_string(), "corrupt".to_string()],
        }
    }
}

/// Monitoring configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MonitoringConfig {
    pub prometheus: PrometheusConfig,
    pub grafana: GrafanaConfig,
    pub alertmanager: AlertmanagerConfig,
    pub vector: VectorConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrometheusConfig {
    pub image: String,
    pub port: u16,
    pub data_dir: String,
    pub config_path: String,
    pub retention_days: u32,
    pub scrape_interval_secs: u32,
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
pub struct GrafanaConfig {
    pub image: String,
    pub port: u16,
    pub data_dir: String,
    pub admin_user: String,
    pub admin_password: String,
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
pub struct AlertmanagerConfig {
    pub image: String,
    pub port: u16,
    pub data_dir: String,
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
pub struct VectorConfig {
    pub image: String,
    pub port: u16,
    pub data_dir: String,
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
    pub storage: StorageLimits,
    pub log_retention: LogRetention,
    pub emergency_thresholds: EmergencyThresholds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageLimits {
    pub validator_data_soft_gb: usize,
    pub validator_data_hard_gb: usize,
    pub validator_logs_soft_mb: usize,
    pub validator_logs_hard_mb: usize,
    pub reports_soft_gb: usize,
    pub reports_hard_gb: usize,
    pub monitoring_soft_gb: usize,
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
pub struct LogRetention {
    pub validator_stdout_days: u32,
    pub consensus_traces_days: u32,
    pub rpc_access_days: u32,
    pub attack_artifacts_days: u32,
    pub metrics_days: u32,
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
pub struct EmergencyThresholds {
    pub host_commit_charge_gb: usize,
    pub host_physical_ram_free_gb: usize,
    pub disk_d_free_gb: usize,
    pub validator_cpu_percent: u8,
    pub validator_ram_mb: usize,
    pub validator_disk_gb: usize,
    pub log_collection_lag_minutes: usize,
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
    pub base_dir: String,
    pub genesis_dir: String,
    pub reports_dir: String,
    pub runs_dir: String,
    pub control_dir: String,
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