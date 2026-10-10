//! Metrics collection and reporting

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;
use tracing::{info, warn};

/// Lab-wide metrics
#[allow(dead_code)]
pub struct LabMetrics {
    registry: Registry,

    // Validator metrics - use Family for labeled metrics
    validator_up: Family<ValidatorLabels, Gauge>,
    validator_height: Family<ValidatorLabels, Gauge>,
    validator_peers: Family<ValidatorLabels, Gauge>,
    validator_memory_mb: Family<ValidatorLabels, Gauge>,
    validator_cpu_percent: Family<ValidatorLabels, Gauge>,

    // Consensus metrics
    blocks_finalized: Family<ConsensusLabels, Counter>,
    consensus_rounds: Family<ConsensusLabels, Counter>,
    forks_detected: Family<ConsensusLabels, Counter>,
    finality_time_ms: Family<ConsensusLabels, Histogram>,

    // Network metrics
    p2p_messages_sent: Family<NetworkLabels, Counter>,
    p2p_messages_received: Family<NetworkLabels, Counter>,
    p2p_errors: Family<NetworkLabels, Counter>,
    p2p_latency_ms: Family<NetworkLabels, Histogram>,

    // RPC metrics
    rpc_requests: Family<RpcLabels, Counter>,
    rpc_errors: Family<RpcLabels, Counter>,
    rpc_latency_ms: Family<RpcLabels, Histogram>,

    // Scenario metrics
    scenario_runs: Family<ScenarioLabels, Counter>,
    scenario_duration_ms: Family<ScenarioLabels, Histogram>,
    scenario_verdict: Family<ScenarioLabels, Counter>,

    // Fault injection metrics
    faults_injected: Family<FaultLabels, Counter>,
    fault_duration_ms: Family<FaultLabels, Histogram>,

    // System metrics (no labels)
    host_memory_usage_mb: Gauge,
    host_cpu_percent: Gauge,
    host_disk_free_gb: Gauge,
    host_commit_charge_gb: Gauge,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ValidatorLabels {
    pub name: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ConsensusLabels {
    pub chain_id: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct NetworkLabels {
    pub direction: String, // "sent" or "received"
    pub peer: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct RpcLabels {
    pub method: String,
    pub status: String, // "success" or "error"
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct ScenarioLabels {
    pub scenario_id: String,
    pub category: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct FaultLabels {
    pub fault_type: String,
    pub target: String,
}

#[allow(dead_code)]
impl LabMetrics {
    pub fn new() -> Self {
        let mut registry = Registry::default();

        let validator_up = Family::<ValidatorLabels, Gauge>::default();
        let validator_height = Family::<ValidatorLabels, Gauge>::default();
        let validator_peers = Family::<ValidatorLabels, Gauge>::default();
        let validator_memory_mb = Family::<ValidatorLabels, Gauge>::default();
        let validator_cpu_percent = Family::<ValidatorLabels, Gauge>::default();

        let blocks_finalized = Family::<ConsensusLabels, Counter>::default();
        let consensus_rounds = Family::<ConsensusLabels, Counter>::default();
        let forks_detected = Family::<ConsensusLabels, Counter>::default();

        // Histogram requires explicit construction with buckets
        fn histogram_constructor() -> Histogram {
            Histogram::new(vec![
                1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
                30000.0, 60000.0, 120000.0, 300000.0, 600000.0,
            ])
        }

        let finality_time_ms =
            Family::<ConsensusLabels, Histogram>::new_with_constructor(histogram_constructor);

        let p2p_messages_sent = Family::<NetworkLabels, Counter>::default();
        let p2p_messages_received = Family::<NetworkLabels, Counter>::default();
        let p2p_errors = Family::<NetworkLabels, Counter>::default();
        let p2p_latency_ms =
            Family::<NetworkLabels, Histogram>::new_with_constructor(histogram_constructor);

        let rpc_requests = Family::<RpcLabels, Counter>::default();
        let rpc_errors = Family::<RpcLabels, Counter>::default();
        let rpc_latency_ms =
            Family::<RpcLabels, Histogram>::new_with_constructor(histogram_constructor);

        let scenario_runs = Family::<ScenarioLabels, Counter>::default();
        let scenario_duration_ms =
            Family::<ScenarioLabels, Histogram>::new_with_constructor(histogram_constructor);
        let scenario_verdict = Family::<ScenarioLabels, Counter>::default();

        let faults_injected = Family::<FaultLabels, Counter>::default();
        let fault_duration_ms =
            Family::<FaultLabels, Histogram>::new_with_constructor(histogram_constructor);

        let host_memory_usage_mb = Gauge::default();
        let host_cpu_percent = Gauge::default();
        let host_disk_free_gb = Gauge::default();
        let host_commit_charge_gb = Gauge::default();

        // Register all metrics
        registry.register(
            "validator_up",
            "Whether validator process is running (1) or not (0)",
            validator_up.clone(),
        );
        registry.register(
            "validator_height",
            "Current block height of validator",
            validator_height.clone(),
        );
        registry.register(
            "validator_peers",
            "Number of connected P2P peers",
            validator_peers.clone(),
        );
        registry.register(
            "validator_memory_mb",
            "Validator memory usage in MB",
            validator_memory_mb.clone(),
        );
        registry.register(
            "validator_cpu_percent",
            "Validator CPU usage percentage",
            validator_cpu_percent.clone(),
        );

        registry.register(
            "blocks_finalized_total",
            "Total number of finalized blocks",
            blocks_finalized.clone(),
        );
        registry.register(
            "consensus_rounds_total",
            "Total number of consensus rounds",
            consensus_rounds.clone(),
        );
        registry.register(
            "forks_detected_total",
            "Total number of forks detected",
            forks_detected.clone(),
        );
        registry.register(
            "finality_time_ms",
            "Time to finality in milliseconds",
            finality_time_ms.clone(),
        );

        registry.register(
            "p2p_messages_sent_total",
            "Total P2P messages sent",
            p2p_messages_sent.clone(),
        );
        registry.register(
            "p2p_messages_received_total",
            "Total P2P messages received",
            p2p_messages_received.clone(),
        );
        registry.register("p2p_errors_total", "Total P2P errors", p2p_errors.clone());
        registry.register(
            "p2p_latency_ms",
            "P2P message latency in milliseconds",
            p2p_latency_ms.clone(),
        );

        registry.register(
            "rpc_requests_total",
            "Total RPC requests",
            rpc_requests.clone(),
        );
        registry.register("rpc_errors_total", "Total RPC errors", rpc_errors.clone());
        registry.register(
            "rpc_latency_ms",
            "RPC latency in milliseconds",
            rpc_latency_ms.clone(),
        );

        registry.register(
            "scenario_runs_total",
            "Total scenario runs",
            scenario_runs.clone(),
        );
        registry.register(
            "scenario_duration_ms",
            "Scenario duration in milliseconds",
            scenario_duration_ms.clone(),
        );
        registry.register(
            "scenario_verdict_total",
            "Scenario verdict counts",
            scenario_verdict.clone(),
        );

        registry.register(
            "faults_injected_total",
            "Total faults injected",
            faults_injected.clone(),
        );
        registry.register(
            "fault_duration_ms",
            "Fault injection duration in milliseconds",
            fault_duration_ms.clone(),
        );

        registry.register(
            "host_memory_usage_mb",
            "Host memory usage in MB",
            host_memory_usage_mb.clone(),
        );
        registry.register(
            "host_cpu_percent",
            "Host CPU usage percentage",
            host_cpu_percent.clone(),
        );
        registry.register(
            "host_disk_free_gb",
            "Host disk D: free space in GB",
            host_disk_free_gb.clone(),
        );
        registry.register(
            "host_commit_charge_gb",
            "Host commit charge in GB",
            host_commit_charge_gb.clone(),
        );

        Self {
            registry,
            validator_up,
            validator_height,
            validator_peers,
            validator_memory_mb,
            validator_cpu_percent,
            blocks_finalized,
            consensus_rounds,
            forks_detected,
            finality_time_ms,
            p2p_messages_sent,
            p2p_messages_received,
            p2p_errors,
            p2p_latency_ms,
            rpc_requests,
            rpc_errors,
            rpc_latency_ms,
            scenario_runs,
            scenario_duration_ms,
            scenario_verdict,
            faults_injected,
            fault_duration_ms,
            host_memory_usage_mb,
            host_cpu_percent,
            host_disk_free_gb,
            host_commit_charge_gb,
        }
    }

    /// Record validator status
    pub fn set_validator_up(&self, name: &str, up: bool) {
        self.validator_up
            .get_or_create(&ValidatorLabels {
                name: name.to_string(),
            })
            .set(if up { 1 } else { 0 });
    }

    /// Record validator height
    pub fn set_validator_height(&self, name: &str, height: u64) {
        self.validator_height
            .get_or_create(&ValidatorLabels {
                name: name.to_string(),
            })
            .set(height as i64);
    }

    /// Record validator peer count
    pub fn set_validator_peers(&self, name: &str, peers: u64) {
        self.validator_peers
            .get_or_create(&ValidatorLabels {
                name: name.to_string(),
            })
            .set(peers as i64);
    }

    /// Record validator memory
    pub fn set_validator_memory_mb(&self, name: &str, mb: u64) {
        self.validator_memory_mb
            .get_or_create(&ValidatorLabels {
                name: name.to_string(),
            })
            .set(mb as i64);
    }

    /// Record validator CPU
    pub fn set_validator_cpu_percent(&self, name: &str, percent: f64) {
        self.validator_cpu_percent
            .get_or_create(&ValidatorLabels {
                name: name.to_string(),
            })
            .set(percent as i64);
    }

    /// Increment finalized blocks
    pub fn inc_blocks_finalized(&self, chain_id: &str, count: u64) {
        for _ in 0..count {
            self.blocks_finalized
                .get_or_create(&ConsensusLabels {
                    chain_id: chain_id.to_string(),
                })
                .inc();
        }
    }

    /// Increment consensus rounds
    pub fn inc_consensus_rounds(&self, chain_id: &str) {
        self.consensus_rounds
            .get_or_create(&ConsensusLabels {
                chain_id: chain_id.to_string(),
            })
            .inc();
    }

    /// Increment forks detected
    pub fn inc_forks_detected(&self, chain_id: &str) {
        self.forks_detected
            .get_or_create(&ConsensusLabels {
                chain_id: chain_id.to_string(),
            })
            .inc();
    }

    /// Observe finality time
    pub fn observe_finality_time(&self, chain_id: &str, ms: f64) {
        self.finality_time_ms
            .get_or_create(&ConsensusLabels {
                chain_id: chain_id.to_string(),
            })
            .observe(ms);
    }

    /// Increment P2P messages
    pub fn inc_p2p_messages(&self, direction: &str, peer: &str) {
        if direction == "sent" {
            self.p2p_messages_sent
                .get_or_create(&NetworkLabels {
                    direction: direction.to_string(),
                    peer: peer.to_string(),
                })
                .inc();
        } else {
            self.p2p_messages_received
                .get_or_create(&NetworkLabels {
                    direction: direction.to_string(),
                    peer: peer.to_string(),
                })
                .inc();
        }
    }

    /// Increment P2P errors
    pub fn inc_p2p_errors(&self, peer: &str) {
        self.p2p_errors
            .get_or_create(&NetworkLabels {
                direction: "error".to_string(),
                peer: peer.to_string(),
            })
            .inc();
    }

    /// Observe P2P latency
    pub fn observe_p2p_latency(&self, peer: &str, ms: f64) {
        self.p2p_latency_ms
            .get_or_create(&NetworkLabels {
                direction: "latency".to_string(),
                peer: peer.to_string(),
            })
            .observe(ms);
    }

    /// Increment RPC requests
    pub fn inc_rpc_requests(&self, method: &str, success: bool) {
        let status = if success { "success" } else { "error" };
        self.rpc_requests
            .get_or_create(&RpcLabels {
                method: method.to_string(),
                status: status.to_string(),
            })
            .inc();

        if !success {
            self.rpc_errors
                .get_or_create(&RpcLabels {
                    method: method.to_string(),
                    status: "error".to_string(),
                })
                .inc();
        }
    }

    /// Observe RPC latency
    pub fn observe_rpc_latency(&self, method: &str, ms: f64) {
        self.rpc_latency_ms
            .get_or_create(&RpcLabels {
                method: method.to_string(),
                status: "latency".to_string(),
            })
            .observe(ms);
    }

    /// Record scenario run
    pub fn record_scenario(
        &self,
        scenario_id: &str,
        category: u8,
        duration_ms: u64,
        _verdict: &str,
    ) {
        self.scenario_runs
            .get_or_create(&ScenarioLabels {
                scenario_id: scenario_id.to_string(),
                category: category.to_string(),
            })
            .inc();

        self.scenario_duration_ms
            .get_or_create(&ScenarioLabels {
                scenario_id: scenario_id.to_string(),
                category: category.to_string(),
            })
            .observe(duration_ms as f64);

        self.scenario_verdict
            .get_or_create(&ScenarioLabels {
                scenario_id: scenario_id.to_string(),
                category: category.to_string(),
            })
            .inc();
    }

    /// Record fault injection
    pub fn record_fault(&self, fault_type: &str, target: &str, duration_ms: u64) {
        self.faults_injected
            .get_or_create(&FaultLabels {
                fault_type: fault_type.to_string(),
                target: target.to_string(),
            })
            .inc();

        self.fault_duration_ms
            .get_or_create(&FaultLabels {
                fault_type: fault_type.to_string(),
                target: target.to_string(),
            })
            .observe(duration_ms as f64);
    }

    /// Update host metrics
    pub fn set_host_memory_mb(&self, mb: u64) {
        self.host_memory_usage_mb.set(mb as i64);
    }

    pub fn set_host_cpu_percent(&self, percent: f64) {
        self.host_cpu_percent.set(percent as i64);
    }

    pub fn set_host_disk_free_gb(&self, gb: u64) {
        self.host_disk_free_gb.set(gb as i64);
    }

    pub fn set_host_commit_charge_gb(&self, gb: u64) {
        self.host_commit_charge_gb.set(gb as i64);
    }

    /// Print summary of current metrics
    pub async fn print_summary(&self) {
        info!("=== Lab Metrics Summary ===");

        let mut buffer = String::new();
        if let Err(error) = prometheus_client::encoding::text::encode(&mut buffer, &self.registry) {
            warn!("metrics summary unavailable: {error}");
            return;
        }

        for line in buffer.lines() {
            if line.contains("validator_up")
                || line.contains("blocks_finalized")
                || line.contains("forks_detected")
                || line.contains("scenario_verdict")
            {
                println!("{}", line);
            }
        }
    }

    /// Get registry for Prometheus scraping
    pub fn registry(&self) -> &Registry {
        &self.registry
    }
}

impl Default for LabMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Metrics collector that periodically scrapes validators
#[allow(dead_code)]
pub struct MetricsCollector {
    metrics: Arc<LabMetrics>,
    config: Arc<crate::config::LabConfig>,
    interval: Duration,
}

#[allow(dead_code)]
impl MetricsCollector {
    pub fn new(
        metrics: Arc<LabMetrics>,
        config: Arc<crate::config::LabConfig>,
        interval: Duration,
    ) -> Self {
        Self {
            metrics,
            config,
            interval,
        }
    }

    pub async fn run(&self) -> Result<()> {
        let mut interval = tokio::time::interval(self.interval);

        loop {
            interval.tick().await;
            self.collect().await?;
        }
    }

    async fn collect(&self) -> Result<()> {
        // Collect validator metrics
        for validator in &self.config.validators {
            // Try to get metrics from validator's /metrics endpoint
            if let Ok(resp) = reqwest::get(&validator.metrics_url()).await
                && let Ok(text) = resp.text().await
            {
                self.parse_prometheus_metrics(&validator.name, &text);
            }

            // Try to get health/status
            if let Ok(resp) = reqwest::get(&validator.health_url()).await {
                if resp.status().is_success() {
                    self.metrics.set_validator_up(&validator.name, true);

                    // Get tip height
                    if let Ok(tip_resp) =
                        reqwest::get(&format!("{}/get_tip_height", validator.rpc_url())).await
                        && let Ok(json) = tip_resp.json::<serde_json::Value>().await
                        && let Some(height) = json.get("result").and_then(|r| r.as_u64())
                    {
                        self.metrics.set_validator_height(&validator.name, height);
                    }
                } else {
                    self.metrics.set_validator_up(&validator.name, false);
                }
            } else {
                self.metrics.set_validator_up(&validator.name, false);
            }
        }

        // Collect host metrics
        self.collect_host_metrics().await?;

        Ok(())
    }

    fn parse_prometheus_metrics(&self, validator: &str, text: &str) {
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }

            // Parse basic metrics
            if let Some((name, value)) = parse_metric_line(line) {
                match name.as_str() {
                    "process_resident_memory_bytes" => {
                        if let Ok(bytes) = value.parse::<u64>() {
                            self.metrics
                                .set_validator_memory_mb(validator, bytes / 1_000_000);
                        }
                    }
                    "process_cpu_seconds_total" => {
                        // Would need rate calculation
                    }
                    _ => {}
                }
            }
        }
    }

    async fn collect_host_metrics(&self) -> Result<()> {
        #[cfg(windows)]
        {
            // Use PowerShell to get system metrics
            let output = tokio::process::Command::new("powershell")
                .args(["-Command",
                    "Get-CimInstance Win32_OperatingSystem | Select-Object FreeVirtualMemory, TotalVirtualMemorySize, FreePhysicalMemory, TotalVisibleMemorySize | ConvertTo-Json"
                ])
                .output()
                .await?;

            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                let free_virt = json["FreeVirtualMemory"].as_u64().unwrap_or(0) / 1024; // MB
                let total_virt = json["TotalVirtualMemorySize"].as_u64().unwrap_or(1) / 1024;
                let free_phys = json["FreePhysicalMemory"].as_u64().unwrap_or(0) / 1024;
                let total_phys = json["TotalVisibleMemorySize"].as_u64().unwrap_or(1) / 1024;

                let used_phys = total_phys.saturating_sub(free_phys);
                let commit_used = total_virt.saturating_sub(free_virt);

                self.metrics.set_host_memory_mb(used_phys);
                self.metrics.set_host_commit_charge_gb(commit_used / 1024);
            }

            // Disk space
            let output = tokio::process::Command::new("powershell")
                .args([
                    "-Command",
                    "Get-PSDrive D | Select-Object Free | ConvertTo-Json",
                ])
                .output()
                .await?;

            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                let free_bytes = json["Free"].as_u64().unwrap_or(0);
                self.metrics
                    .set_host_disk_free_gb(free_bytes / 1_000_000_000);
            }

            // CPU
            let output = tokio::process::Command::new("powershell")
                .args(["-Command", "Get-Counter '\\Processor(_Total)\\% Processor Time' | Select-Object -ExpandProperty CounterSamples | Select-Object -ExpandProperty CookedValue | ConvertTo-Json"])
                .output()
                .await?;

            if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout)
                && let Some(cpu) = json.as_f64()
            {
                self.metrics.set_host_cpu_percent(cpu);
            }
        }

        Ok(())
    }
}

#[allow(dead_code)]
fn parse_metric_line(line: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}
