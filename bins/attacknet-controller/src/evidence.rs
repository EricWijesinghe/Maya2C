//! Evidence collection and reporting

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

/// Scenario verdict
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    /// Set when no check fails. Returned by `verify_success_criteria`.
    Pass,
    /// Set when a fork is found or a scenario-specific check fails.
    Fail,
    /// The initial value when a run starts; stays until `finalize` sets a verdict.
    Incomplete,
    /// Reserved for callers. This crate never sets it.
    Skipped,
    /// Reserved for callers. This crate never sets it.
    Aborted,
}

impl Verdict {
    /// Upper-case label written into reports (`PASS`, `FAIL`, ...).
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Incomplete => "INCOMPLETE",
            Verdict::Skipped => "SKIPPED",
            Verdict::Aborted => "ABORTED",
        }
    }
}

/// Severity levels for findings
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    /// Most serious level. Scenario definitions set it as `severity_if_failed` for their most important checks.
    Critical,
    /// Serious, below `Critical`.
    High,
    /// Moderate impact.
    Medium,
    /// Minor impact.
    Low,
    /// Informational; no impact on the network.
    Info,
}

impl Severity {
    /// Upper-case label written into reports (`CRITICAL`, `HIGH`, ...).
    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Critical => "CRITICAL",
            Severity::High => "HIGH",
            Severity::Medium => "MEDIUM",
            Severity::Low => "LOW",
            Severity::Info => "INFO",
        }
    }
}

/// A finding discovered during a scenario
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Identifier of the finding, unique within its run.
    pub id: String,
    /// How serious the finding is.
    pub severity: Severity,
    /// Free-text grouping label, shown in the report.
    pub category: String,
    /// One-line summary.
    pub title: String,
    /// Full description of what was observed.
    pub description: String,
    /// Names of the validators, services or segments involved.
    pub affected_components: Vec<String>,
    /// Steps that reproduce the finding, in order.
    pub reproduction_steps: Vec<String>,
    /// Paths or identifiers of the evidence supporting the finding.
    pub evidence_refs: Vec<String>,
    /// Unix time of the observation, in seconds. Set by the caller; this crate builds no `Finding` itself.
    pub timestamp: u64,
}

/// Evidence collected for a single scenario run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioEvidence {
    /// Scenario identifier. Also the last directory name under `runs/<date>/`.
    pub scenario_id: String,
    /// Human-readable scenario name.
    pub scenario_name: String,
    /// Scenario category number, as passed to `start_scenario`.
    pub category: u8,
    /// Random v4 UUID generated when the run starts.
    pub run_id: String,
    /// Run start, in Unix seconds.
    pub start_time: u64,
    /// Run end, in Unix seconds. `None` until `finalize` runs.
    pub end_time: Option<u64>,
    /// Run length in milliseconds. Computed from whole-second timestamps, so it is always a multiple of 1000.
    pub duration_ms: Option<u64>,
    /// Outcome, set by `finalize`.
    pub verdict: Verdict,
    /// Findings added with `add_finding`.
    pub findings: Vec<Finding>,
    /// Numbers supplied with `update_metrics`.
    pub metrics: ScenarioMetrics,
    /// Snapshot of the lab and build the run used.
    pub configuration: ScenarioConfig,
    /// Paths to the logs of each component.
    pub logs: LogReferences,
    /// Files produced by the run, added with `add_artifact`.
    pub artifacts: Vec<ArtifactRef>,
}

/// Metrics captured during scenario
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
/// Numbers for one run. Each value is supplied by the caller through `update_metrics`; this crate does not compute them.
pub struct ScenarioMetrics {
    /// Blocks finalized.
    pub blocks_finalized: u64,
    /// Transactions processed.
    pub transactions_processed: u64,
    /// Consensus rounds.
    pub consensus_rounds: u64,
    /// Forks.
    pub forks_detected: u64,
    /// Validator crashes.
    pub validator_crashes: u64,
    /// Validator restarts.
    pub validator_restarts: u64,
    /// Network partitions.
    pub network_partitions: u64,
    /// RPC errors.
    pub rpc_errors: u64,
    /// p2p errors.
    pub p2p_errors: u64,
    /// Storage errors.
    pub storage_errors: u64,
    /// Average block time, in milliseconds.
    pub avg_block_time_ms: f64,
    /// Average finality time, in milliseconds.
    pub avg_finality_time_ms: f64,
    /// Peak memory use, in MiB.
    pub peak_memory_mb: u64,
    /// Peak CPU use, in percent.
    pub peak_cpu_percent: f64,
    /// Disk growth, in MiB.
    pub disk_growth_mb: u64,
    /// Network throughput, in megabits per second.
    pub network_throughput_mbps: f64,
}

/// Scenario configuration snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioConfig {
    /// Lab identifier from `LabConfig::lab_id`.
    pub lab_id: String,
    /// Chain identifier from `LabConfig::chain_id`.
    pub chain_id: String,
    /// Git commit the run was built from.
    pub git_commit: String,
    /// Git branch the run was built from.
    pub git_branch: String,
    /// Binary name to checksum. The scenario runner currently leaves this empty.
    pub binary_checksums: HashMap<String, String>,
    /// Lowercase hex SHA-256 of the text of `genesis.json`.
    pub genesis_hash: String,
    /// Identities of the validators in the run.
    pub validator_identities: Vec<String>,
    /// The scenario's fault specification, serialized as JSON.
    pub fault_injection: serde_json::Value,
    /// Random value drawn for this run and recorded. No generator in this crate is seeded from it, so it does not reproduce the run.
    pub random_seed: u64,
}

/// Log file references
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LogReferences {
    /// Validator name to the path of its log file.
    pub validator_logs: HashMap<String, String>,
    /// Infrastructure service name to the path of its log file.
    pub infrastructure_logs: HashMap<String, String>,
    /// Path to the controller's own log.
    pub controller_log: String,
    /// Paths to packet captures taken during the run.
    pub packet_captures: Vec<String>,
}

/// Artifact reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// File name, for display.
    pub name: String,
    /// Path to the file.
    pub path: String,
    /// Checksum of the file as recorded by the caller. Algorithm is not fixed by this type.
    pub checksum: String,
    /// File size, in bytes.
    pub size_bytes: u64,
    /// Kind of file.
    pub artifact_type: ArtifactType,
}

/// Kind of file recorded as an artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ArtifactType {
    /// Text log.
    Log,
    /// Metrics export.
    Metrics,
    /// Packet capture.
    PacketCapture,
    /// Snapshot of node state.
    StateSnapshot,
    /// Dump of a node's database.
    DatabaseDump,
    /// Generated report.
    Report,
    /// Configuration file.
    Config,
    /// Any other kind; the label is shown in reports as given.
    Other(
        /// Label shown in the report.
        String,
    ),
}

/// Evidence collector: one scenario run at a time.
pub struct EvidenceCollector {
    base_dir: PathBuf,
    current_run: Option<ScenarioEvidence>,
}

impl EvidenceCollector {
    /// Creates a collector with no active run. `base_dir` is where `finalize` writes `runs/<date>/<scenario_id>/evidence.json`.
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            current_run: None,
        }
    }

    /// Start collecting evidence for a scenario
    pub fn start_scenario(
        &mut self,
        scenario_id: &str,
        scenario_name: &str,
        category: u8,
        config: ScenarioConfig,
    ) -> Result<()> {
        let run_id = Uuid::new_v4().to_string();
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());

        let evidence = ScenarioEvidence {
            scenario_id: scenario_id.to_string(),
            scenario_name: scenario_name.to_string(),
            category,
            run_id,
            start_time,
            end_time: None,
            duration_ms: None,
            verdict: Verdict::Incomplete,
            findings: Vec::new(),
            metrics: ScenarioMetrics::default(),
            configuration: config,
            logs: LogReferences::default(),
            artifacts: Vec::new(),
        };

        self.current_run = Some(evidence);
        Ok(())
    }

    /// Add a finding to current scenario
    #[allow(dead_code)]
    pub fn add_finding(&mut self, finding: Finding) -> Result<()> {
        if let Some(run) = &mut self.current_run {
            run.findings.push(finding);
        }
        Ok(())
    }

    /// Update metrics for current scenario
    #[allow(dead_code)]
    pub fn update_metrics(&mut self, metrics: ScenarioMetrics) -> Result<()> {
        if let Some(run) = &mut self.current_run {
            run.metrics = metrics;
        }
        Ok(())
    }

    /// Add log reference
    #[allow(dead_code)]
    pub fn add_log_ref(&mut self, component: &str, path: &str) -> Result<()> {
        if let Some(run) = &mut self.current_run {
            run.logs
                .validator_logs
                .insert(component.to_string(), path.to_string());
        }
        Ok(())
    }

    /// Add artifact reference
    #[allow(dead_code)]
    pub fn add_artifact(&mut self, artifact: ArtifactRef) -> Result<()> {
        if let Some(run) = &mut self.current_run {
            run.artifacts.push(artifact);
        }
        Ok(())
    }

    /// Finalize and save scenario evidence
    pub fn finalize(&mut self, verdict: Verdict) -> Result<ScenarioEvidence> {
        let mut run = self
            .current_run
            .take()
            .ok_or_else(|| anyhow::anyhow!("No active scenario run"))?;

        let end_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());

        run.end_time = Some(end_time);
        run.duration_ms = Some((end_time - run.start_time) * 1000);
        run.verdict = verdict;

        // Save to file
        let date = chrono::DateTime::from_timestamp(run.start_time as i64, 0)
            .unwrap_or_default()
            .format("%Y-%m-%d")
            .to_string();

        let run_dir = self
            .base_dir
            .join("runs")
            .join(&date)
            .join(&run.scenario_id);
        std::fs::create_dir_all(&run_dir).context("Creating run directory")?;

        let evidence_path = run_dir.join("evidence.json");
        let json = serde_json::to_string_pretty(&run).context("Serializing evidence")?;
        std::fs::write(&evidence_path, json).context("Writing evidence file")?;

        // Also save a human-readable summary
        let summary_path = run_dir.join("summary.md");
        let summary = self.generate_summary(&run);
        std::fs::write(&summary_path, summary).context("Writing summary file")?;

        info!("Evidence saved to {}", run_dir.display());

        Ok(run)
    }

    /// Generate human-readable summary
    fn generate_summary(&self, run: &ScenarioEvidence) -> String {
        let start = chrono::DateTime::from_timestamp(run.start_time as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_default();

        let end = run
            .end_time
            .map(|t| {
                chrono::DateTime::from_timestamp(t as i64, 0)
                    .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        let mut summary = String::new();
        summary.push_str(&format!("# Scenario Evidence: {}\n\n", run.scenario_name));
        summary.push_str(&format!("- **Scenario ID**: {}\n", run.scenario_id));
        summary.push_str(&format!("- **Category**: {}\n", run.category));
        summary.push_str(&format!("- **Run ID**: {}\n", run.run_id));
        summary.push_str(&format!("- **Start**: {}\n", start));
        summary.push_str(&format!("- **End**: {}\n", end));
        summary.push_str(&format!(
            "- **Duration**: {} ms\n",
            run.duration_ms.unwrap_or(0)
        ));
        summary.push_str(&format!("- **Verdict**: **{}**\n\n", run.verdict.as_str()));

        summary.push_str("## Metrics\n\n");
        summary.push_str(&format!(
            "- Blocks Finalized: {}\n",
            run.metrics.blocks_finalized
        ));
        summary.push_str(&format!(
            "- Transactions Processed: {}\n",
            run.metrics.transactions_processed
        ));
        summary.push_str(&format!(
            "- Consensus Rounds: {}\n",
            run.metrics.consensus_rounds
        ));
        summary.push_str(&format!(
            "- Forks Detected: {}\n",
            run.metrics.forks_detected
        ));
        summary.push_str(&format!(
            "- Validator Crashes: {}\n",
            run.metrics.validator_crashes
        ));
        summary.push_str(&format!(
            "- Validator Restarts: {}\n",
            run.metrics.validator_restarts
        ));
        summary.push_str(&format!(
            "- Network Partitions: {}\n",
            run.metrics.network_partitions
        ));
        summary.push_str(&format!("- RPC Errors: {}\n", run.metrics.rpc_errors));
        summary.push_str(&format!("- P2P Errors: {}\n", run.metrics.p2p_errors));
        summary.push_str(&format!(
            "- Storage Errors: {}\n",
            run.metrics.storage_errors
        ));
        summary.push_str(&format!(
            "- Avg Block Time: {:.1} ms\n",
            run.metrics.avg_block_time_ms
        ));
        summary.push_str(&format!(
            "- Avg Finality Time: {:.1} ms\n",
            run.metrics.avg_finality_time_ms
        ));
        summary.push_str(&format!(
            "- Peak Memory: {} MB\n",
            run.metrics.peak_memory_mb
        ));
        summary.push_str(&format!(
            "- Peak CPU: {:.1}%\n",
            run.metrics.peak_cpu_percent
        ));
        summary.push_str(&format!(
            "- Disk Growth: {} MB\n",
            run.metrics.disk_growth_mb
        ));
        summary.push_str(&format!(
            "- Network Throughput: {:.1} Mbps\n\n",
            run.metrics.network_throughput_mbps
        ));

        if !run.findings.is_empty() {
            summary.push_str("## Findings\n\n");
            for finding in &run.findings {
                summary.push_str(&format!(
                    "### {} [{}]\n",
                    finding.title,
                    finding.severity.as_str()
                ));
                summary.push_str(&format!("**Category**: {}\n", finding.category));
                summary.push_str(&format!("**Description**: {}\n", finding.description));
                summary.push_str(&format!(
                    "**Affected**: {}\n",
                    finding.affected_components.join(", ")
                ));
                summary.push_str("**Steps**:\n");
                for step in &finding.reproduction_steps {
                    summary.push_str(&format!("1. {}\n", step));
                }
                summary.push('\n');
            }
        }

        summary.push_str("## Configuration\n\n");
        summary.push_str(&format!("- Lab ID: {}\n", run.configuration.lab_id));
        summary.push_str(&format!("- Chain ID: {}\n", run.configuration.chain_id));
        summary.push_str(&format!("- Git Commit: {}\n", run.configuration.git_commit));
        summary.push_str(&format!("- Git Branch: {}\n", run.configuration.git_branch));
        summary.push_str(&format!(
            "- Genesis Hash: {}\n",
            run.configuration.genesis_hash
        ));
        summary.push_str(&format!(
            "- Validators: {}\n",
            run.configuration.validator_identities.join(", ")
        ));
        summary.push_str(&format!(
            "- Random Seed: {}\n\n",
            run.configuration.random_seed
        ));

        summary.push_str("## Artifacts\n\n");
        for artifact in &run.artifacts {
            summary.push_str(&format!(
                "- **{}** ({}) - {} bytes - {}\n",
                artifact.name,
                match &artifact.artifact_type {
                    ArtifactType::Log => "Log",
                    ArtifactType::Metrics => "Metrics",
                    ArtifactType::PacketCapture => "Packet Capture",
                    ArtifactType::StateSnapshot => "State Snapshot",
                    ArtifactType::DatabaseDump => "Database Dump",
                    ArtifactType::Report => "Report",
                    ArtifactType::Config => "Config",
                    ArtifactType::Other(s) => s,
                },
                artifact.size_bytes,
                artifact.checksum
            ));
        }

        summary
    }

    /// Get current run (for inspection)
    #[allow(dead_code)]
    pub fn current_run(&self) -> Option<&ScenarioEvidence> {
        self.current_run.as_ref()
    }
}

/// Load evidence from a previous run
pub fn load_evidence(path: &Path) -> Result<ScenarioEvidence> {
    let content = std::fs::read_to_string(path).context("Reading evidence file")?;
    let evidence: ScenarioEvidence =
        serde_json::from_str(&content).context("Parsing evidence JSON")?;
    Ok(evidence)
}

/// Generate aggregate report from multiple runs
pub fn generate_aggregate_report(runs: &[ScenarioEvidence], output: &Path) -> Result<()> {
    let mut report = String::new();
    report.push_str("# Attacknet Aggregate Report\n\n");
    report.push_str(&format!(
        "Generated: {}\n\n",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    ));

    // Summary table
    report.push_str("## Scenario Results\n\n");
    report.push_str("| Scenario | Category | Verdict | Duration | Blocks | Forks | Findings |\n");
    report.push_str("|----------|----------|---------|----------|--------|-------|----------|\n");

    for run in runs {
        let findings_crit = run
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Critical)
            .count();
        let findings_high = run
            .findings
            .iter()
            .filter(|f| f.severity == Severity::High)
            .count();
        let findings_med = run
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Medium)
            .count();

        report.push_str(&format!(
            "| {} | {} | {} | {} ms | {} | {} | C{} H{} M{} |\n",
            run.scenario_id,
            run.category,
            run.verdict.as_str(),
            run.duration_ms.unwrap_or(0),
            run.metrics.blocks_finalized,
            run.metrics.forks_detected,
            findings_crit,
            findings_high,
            findings_med
        ));
    }

    report.push_str("\n## Critical Findings\n\n");
    for run in runs {
        for finding in &run.findings {
            if finding.severity == Severity::Critical {
                report.push_str(&format!("### {} ({})\n", finding.title, run.scenario_id));
                report.push_str(&format!("{}\n\n", finding.description));
            }
        }
    }

    report.push_str("## High Severity Findings\n\n");
    for run in runs {
        for finding in &run.findings {
            if finding.severity == Severity::High {
                report.push_str(&format!("### {} ({})\n", finding.title, run.scenario_id));
                report.push_str(&format!("{}\n\n", finding.description));
            }
        }
    }

    std::fs::write(output, report).context("Writing aggregate report")?;

    Ok(())
}
