//! Fault injection for attacknet scenarios

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::LabConfig;

/// Fault injector for various fault types
pub struct FaultInjector {
    config: LabConfig,
}

impl FaultInjector {
    pub fn new(config: LabConfig) -> Self {
        Self { config }
    }

    /// Inject a process fault
    pub async fn inject_process_fault(&self, fault: ProcessFault) -> Result<()> {
        info!("Injecting process fault: {:?}", fault);

        match fault {
            ProcessFault::Kill { target } => self.kill_process(&target).await,
            ProcessFault::Restart {
                target,
                catch_up_from,
            } => self.restart_process(&target, catch_up_from).await,
            ProcessFault::Pause { target, duration } => self.pause_process(&target, duration).await,
            ProcessFault::CpuLimit { target, percent } => self.limit_cpu(&target, percent).await,
            ProcessFault::MemoryLimit { target, limit_mb } => {
                self.limit_memory(&target, limit_mb).await
            }
        }
    }

    /// Inject a network fault
    pub async fn inject_network_fault(&self, fault: NetworkFault) -> Result<()> {
        info!("Injecting network fault: {:?}", fault);

        match fault {
            NetworkFault::Latency {
                segment,
                ms,
                jitter_ms,
            } => self.inject_latency(&segment, ms, jitter_ms).await,
            NetworkFault::PacketLoss { segment, percent } => {
                self.inject_packet_loss(&segment, percent).await
            }
            NetworkFault::BandwidthLimit { segment, kbps } => {
                self.inject_bandwidth_limit(&segment, kbps).await
            }
            NetworkFault::Partition { segment, direction } => {
                self.inject_partition(&segment, direction).await
            }
            NetworkFault::ConnectionReset {
                segment,
                after_bytes,
            } => self.inject_connection_reset(&segment, after_bytes).await,
            NetworkFault::Duplicate { segment, percent } => {
                self.inject_duplication(&segment, percent).await
            }
            NetworkFault::Reorder {
                segment,
                percent,
                gap,
            } => self.inject_reorder(&segment, percent, gap).await,
            NetworkFault::Corruption { segment, percent } => {
                self.inject_corruption(&segment, percent).await
            }
            NetworkFault::SlowSend {
                segment,
                bytes_per_sec,
            } => self.inject_slow_send(&segment, bytes_per_sec).await,
        }
    }

    /// Inject a storage fault
    pub async fn inject_storage_fault(&self, fault: StorageFault) -> Result<()> {
        info!("Injecting storage fault: {:?}", fault);

        match fault {
            StorageFault::DiskFull { target, percent } => self.fill_disk(&target, percent).await,
            StorageFault::ReadOnly { target } => self.make_readonly(&target).await,
            StorageFault::CorruptFile {
                target,
                file_pattern,
                corruption_type,
            } => {
                self.corrupt_file(&target, &file_pattern, corruption_type)
                    .await
            }
            StorageFault::TruncateFile {
                target,
                file_pattern,
                percent,
            } => self.truncate_file(&target, &file_pattern, percent).await,
            StorageFault::DeleteFile {
                target,
                file_pattern,
            } => self.delete_file(&target, &file_pattern).await,
            StorageFault::QuotaExceeded { target } => self.enforce_quota(&target).await,
        }
    }

    /// Inject a protocol-level fault
    pub async fn inject_protocol_fault(&self, fault: ProtocolFault) -> Result<()> {
        info!("Injecting protocol fault: {:?}", fault);

        match fault {
            ProtocolFault::MalformedFrames { target, rate } => {
                self.send_malformed_frames(&target, rate).await
            }
            ProtocolFault::InvalidHandshakes { target, rate } => {
                self.send_invalid_handshakes(&target, rate).await
            }
            ProtocolFault::OversizedMessages { target, size_mb } => {
                self.send_oversized_messages(&target, size_mb).await
            }
            ProtocolFault::PeerChurn {
                target,
                rate_per_sec,
            } => self.peer_churn(&target, rate_per_sec).await,
            ProtocolFault::InvalidPeerAnnouncements { target } => {
                self.send_invalid_peer_announcements(&target).await
            }
        }
    }

    // Process fault implementations
    /// The lab data directory of `target`. Faults select processes by this
    /// directory on their command line, never by image name: the live
    /// testnet's seed is also `maya2c-node.exe`, and a name match killed it
    /// on 2026-10-09. An unknown target is an error, not a silent no-op.
    fn data_dir_of(&self, target: &str) -> Result<&str> {
        self.config
            .validators
            .iter()
            .find(|v| v.name == target)
            .map(|v| v.data_dir.as_str())
            .with_context(|| format!("unknown fault target {target}"))
    }

    async fn kill_process(&self, target: &str) -> Result<()> {
        let dir = self.data_dir_of(target)?;
        #[cfg(windows)]
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                // This PowerShell's own command line contains `$d`; without
                // the $PID exclusion it matched itself, so a kill that hit no
                // validator still "succeeded".
                &format!(
                    "$d = '{}'; $p = @(Get-CimInstance Win32_Process | \
                     Where-Object {{ $_.ProcessId -ne $PID -and $_.CommandLine -and $_.CommandLine.Contains($d) }}); \
                     if ($p.Count -eq 0) {{ exit {NO_MATCH_EXIT} }}; \
                     $p | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction Stop }}",
                    dir.replace('\'', "''")
                ),
            ])
            .output()
            .context("Stopping the target's processes")?;
        #[cfg(not(windows))]
        let output = Command::new("pkill")
            .args(["-f", "--", dir])
            .output()
            .context("Running pkill")?;
        if output.status.code() == Some(NO_MATCH_EXIT) {
            anyhow::bail!("kill {target}: no process matched {dir}; nothing was killed");
        }
        if !output.status.success() {
            anyhow::bail!(
                "killing {target} ({dir}) failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    async fn restart_process(&self, target: &str, catch_up_from: Option<String>) -> Result<()> {
        // Logging Ok here let restart scenarios "pass" with no restart.
        anyhow::bail!(
            "restart fault not implemented (target {target}, catch_up_from {catch_up_from:?}); \
             use cargo xtask attacknet"
        )
    }

    async fn pause_process(&self, target: &str, _duration: Duration) -> Result<()> {
        // Lowering the priority class (the previous Windows body) does not
        // pause anything, and it selected processes by image name; the Unix
        // body passed `$(pgrep ...)` to kill without a shell. A fault that
        // did not happen must not let a scenario report PASS.
        anyhow::bail!("pause fault not implemented (target {target})")
    }

    async fn limit_cpu(&self, target: &str, percent: u8) -> Result<()> {
        // Needs Job Objects on Windows or cgroups on Linux.
        anyhow::bail!("CPU-limit fault not implemented ({percent}% on {target})")
    }

    async fn limit_memory(&self, target: &str, limit_mb: usize) -> Result<()> {
        // Needs Job Objects on Windows or cgroups on Linux.
        anyhow::bail!("memory-limit fault not implemented ({limit_mb} MB on {target})")
    }

    // Network fault implementations
    async fn inject_latency(&self, segment: &str, ms: u32, jitter_ms: u32) -> Result<()> {
        // Use pktmon or toxiproxy
        // Needs a proxy (toxiproxy) or WSL `tc netem`; until one exists the
        // fault fails rather than logging success it never had.
        anyhow::bail!("latency fault not implemented ({ms}ms ± {jitter_ms}ms on {segment})")
    }

    async fn inject_packet_loss(&self, segment: &str, percent: f32) -> Result<()> {
        anyhow::bail!("packet-loss fault not implemented ({percent} on {segment})")
    }

    async fn inject_bandwidth_limit(&self, segment: &str, kbps: u32) -> Result<()> {
        anyhow::bail!("bandwidth fault not implemented ({kbps} kbps on {segment})")
    }

    async fn inject_partition(&self, segment: &str, direction: PartitionDirection) -> Result<()> {
        info!("Injecting partition on segment {segment} ({direction:?})");
        let ports = self.segment_ports(segment)?;
        let dirs: &[(&str, &str, &str)] = match direction {
            PartitionDirection::Bidirectional => &[
                ("Bidirectional", "in", "localport"),
                ("Bidirectional_Out", "out", "remoteport"),
            ],
            PartitionDirection::Inbound => &[("Inbound", "in", "localport")],
            PartitionDirection::Outbound => &[("Outbound", "out", "remoteport")],
        };
        info!("Blocking {direction:?} traffic on ports: {ports:?}");
        for port in &ports {
            for (suffix, dir, port_key) in dirs {
                let rule = format!("name=Maya2C_Block_Port_{port}_{suffix}");
                netsh(&["advfirewall", "firewall", "delete", "rule", &rule], true)?;
                netsh(
                    &[
                        "advfirewall",
                        "firewall",
                        "add",
                        "rule",
                        &rule,
                        &format!("dir={dir}"),
                        "action=block",
                        "protocol=TCP",
                        &format!("{port_key}={port}"),
                    ],
                    false,
                )?;
            }
        }
        Ok(())
    }

    /// The P2P ports a partition of `segment` blocks. An unknown segment is
    /// an error: returning Ok let a scenario run "partitioned" with nothing
    /// blocked.
    fn segment_ports(&self, segment: &str) -> Result<Vec<u16>> {
        if !self.config.network.segments.contains_key(segment) {
            anyhow::bail!("unknown network segment {segment}");
        }
        let mut ports = Vec::new();
        if matches!(segment, "S1" | "S2" | "S3" | "S6" | "S7") {
            ports.extend(self.config.validators.iter().map(|v| v.p2p_port));
        }
        ports.extend(self.config.infrastructure.bootnode.p2p_port);
        ports.extend(self.config.infrastructure.adversarial_peer.p2p_port);
        ports.sort_unstable();
        ports.dedup();
        Ok(ports)
    }

    async fn inject_connection_reset(&self, segment: &str, after_bytes: Option<u64>) -> Result<()> {
        anyhow::bail!(
            "connection-reset fault not implemented (segment {segment}, after {after_bytes:?} bytes)"
        )
    }

    async fn inject_duplication(&self, segment: &str, percent: f32) -> Result<()> {
        anyhow::bail!("duplication fault not implemented ({percent} on {segment})")
    }

    async fn inject_reorder(&self, segment: &str, percent: f32, gap: u32) -> Result<()> {
        anyhow::bail!("reorder fault not implemented ({percent}, gap {gap} on {segment})")
    }

    async fn inject_corruption(&self, segment: &str, percent: f32) -> Result<()> {
        anyhow::bail!("packet-corruption fault not implemented ({percent} on {segment})")
    }

    async fn inject_slow_send(&self, segment: &str, bytes_per_sec: u32) -> Result<()> {
        anyhow::bail!("slow-send fault not implemented ({bytes_per_sec} B/s on {segment})")
    }

    pub async fn heal_partition(&self, segment: &str) -> Result<()> {
        info!("Healing partition on segment {segment}");
        let ports = self.segment_ports(segment)?;
        for port in &ports {
            for suffix in ["Bidirectional", "Bidirectional_Out", "Inbound", "Outbound"] {
                let rule = format!("name=Maya2C_Block_Port_{port}_{suffix}");
                netsh(&["advfirewall", "firewall", "delete", "rule", &rule], true)?;
            }
        }
        info!("Removed firewall rules for ports: {ports:?}");
        Ok(())
    }

    // Storage fault implementations
    async fn fill_disk(&self, target: &str, percent: f32) -> Result<()> {
        info!("Filling disk to {}% for {}", percent * 100.0, target);

        // Create a large file to simulate disk pressure
        let data_dir = format!("D:\\Maya2C-attacknet-{}", target);
        let fill_file = Path::new(&data_dir).join("DISK_PRESSURE_TEST.tmp");

        // Calculate size to write
        let total_space = 5_000_000_000u64; // 5 GB quota
        let target_bytes = (total_space as f64 * percent as f64) as u64;

        // Write in chunks
        let chunk_size = 100_000_000u64; // 100 MB chunks
        let mut written = 0u64;
        let data = vec![0u8; chunk_size as usize];

        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&fill_file)?;

        while written < target_bytes {
            file.write_all(&data)?;
            written += chunk_size;
            if written.is_multiple_of(1_000_000_000) {
                info!(
                    "Written {} GB for disk pressure test",
                    written / 1_000_000_000
                );
            }
        }

        info!("Disk pressure test file created: {} bytes", written);
        Ok(())
    }

    async fn make_readonly(&self, target: &str) -> Result<()> {
        info!("Making data directory read-only for {}", target);

        #[cfg(windows)]
        {
            let data_dir = format!("D:\\Maya2C-attacknet-{}", target);
            let output = Command::new("icacls")
                .args([&data_dir, "/deny", "Everyone:(W)"])
                .output()
                .context("Setting read-only ACL")?;
            if !output.status.success() {
                warn!("icacls failed: {}", String::from_utf8_lossy(&output.stderr));
            }
            Ok(())
        }
        // The lab runs on Windows (icacls); elsewhere this fault is unavailable.
        #[cfg(not(windows))]
        anyhow::bail!("make_readonly is only implemented on Windows (icacls)");
    }

    async fn corrupt_file(
        &self,
        target: &str,
        file_pattern: &str,
        corruption_type: CorruptionType,
    ) -> Result<()> {
        info!(
            "Corrupting file matching '{}' for {} ({:?})",
            file_pattern, target, corruption_type
        );
        // Rejected before any file is read, so the fault fails even when no
        // file matches (it used to rewrite matched files unchanged and pass).
        if let CorruptionType::ReplacePattern { .. } = corruption_type {
            anyhow::bail!("replace-pattern corruption not implemented (target {target})");
        }

        let data_dir_path = format!("D:\\Maya2C-attacknet-{}", target);
        let data_dir = Path::new(&data_dir_path);

        for entry in std::fs::read_dir(data_dir)? {
            let entry = entry?;
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.contains(file_pattern) {
                let path = entry.path();
                let mut content = std::fs::read(&path)?;

                match corruption_type {
                    CorruptionType::BitFlip { probability } => {
                        for byte in &mut content {
                            if rand::random::<f32>() < probability {
                                *byte ^= 1 << (rand::random::<u8>() % 8);
                            }
                        }
                    }
                    CorruptionType::Truncate { percent } => {
                        let new_len = (content.len() as f64 * (1.0 - percent as f64)) as usize;
                        content.truncate(new_len);
                    }
                    CorruptionType::ZeroOut {
                        range_start,
                        range_end,
                    } => {
                        let start = (content.len() as f64 * range_start as f64) as usize;
                        let end = (content.len() as f64 * range_end as f64)
                            .min(content.len() as f64) as usize;
                        for byte in content
                            .iter_mut()
                            .skip(start)
                            .take(end.saturating_sub(start))
                        {
                            *byte = 0;
                        }
                    }
                    CorruptionType::ReplacePattern { .. } => unreachable!("rejected above"),
                }

                std::fs::write(&path, content)?;
                warn!("Corrupted file: {}", path.display());
            }
        }

        Ok(())
    }

    async fn truncate_file(&self, target: &str, file_pattern: &str, percent: f32) -> Result<()> {
        self.corrupt_file(target, file_pattern, CorruptionType::Truncate { percent })
            .await
    }

    async fn delete_file(&self, target: &str, file_pattern: &str) -> Result<()> {
        info!("Deleting file matching '{}' for {}", file_pattern, target);

        let data_dir_path = format!("D:\\Maya2C-attacknet-{}", target);
        let data_dir = Path::new(&data_dir_path);

        for entry in std::fs::read_dir(data_dir)? {
            let entry = entry?;
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.contains(file_pattern) {
                std::fs::remove_file(entry.path())?;
                warn!("Deleted file: {}", entry.path().display());
            }
        }

        Ok(())
    }

    async fn enforce_quota(&self, target: &str) -> Result<()> {
        info!("Enforcing NTFS quota for {}", target);

        #[cfg(windows)]
        {
            let data_dir = format!("D:\\Maya2C-attacknet-{}", target);
            let output = Command::new("fsutil")
                .args(["quota", "enforce", &data_dir])
                .output()
                .context("Enforcing quota")?;
            if !output.status.success() {
                warn!(
                    "fsutil quota enforce failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }

        Ok(())
    }

    // Protocol fault implementations
    // Protocol faults need an adversarial peer process that does not exist;
    // each previously logged Ok, so a scenario "passed" with nothing sent.
    async fn send_malformed_frames(&self, target: &str, rate: u32) -> Result<()> {
        anyhow::bail!("malformed-frames fault not implemented ({rate}/s to {target})")
    }

    async fn send_invalid_handshakes(&self, target: &str, rate: u32) -> Result<()> {
        anyhow::bail!("invalid-handshake fault not implemented ({rate}/s to {target})")
    }

    async fn send_oversized_messages(&self, target: &str, size_mb: u32) -> Result<()> {
        anyhow::bail!("oversized-message fault not implemented ({size_mb} MB to {target})")
    }

    async fn peer_churn(&self, target: &str, rate_per_sec: u32) -> Result<()> {
        anyhow::bail!("peer-churn fault not implemented ({rate_per_sec}/s against {target})")
    }

    async fn send_invalid_peer_announcements(&self, target: &str) -> Result<()> {
        anyhow::bail!("invalid-peer-announcement fault not implemented (to {target})")
    }
}

/// Exit code of the kill command when no process matched: pkill's own on
/// Unix; on Windows a code PowerShell does not use for its own errors.
#[cfg(not(windows))]
const NO_MATCH_EXIT: i32 = 1;
#[cfg(windows)]
const NO_MATCH_EXIT: i32 = 3;

/// Runs netsh and fails on a non-zero exit. `missing_ok` accepts deleting a
/// rule that does not exist.
// shortcut: matches netsh's English "No rules match" text; on a non-English
// Windows a delete of an absent rule fails loudly instead, which is safe.
fn netsh(args: &[&str], missing_ok: bool) -> Result<()> {
    let out = Command::new("netsh")
        .args(args)
        .output()
        .context("running netsh")?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if out.status.success() || (missing_ok && stdout.contains("No rules match")) {
        return Ok(());
    }
    anyhow::bail!(
        "netsh {} failed ({}): {} {}",
        args.join(" "),
        out.status,
        stdout.trim(),
        String::from_utf8_lossy(&out.stderr).trim()
    )
}

/// Process fault types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProcessFault {
    Kill {
        target: String,
    },
    Restart {
        target: String,
        catch_up_from: Option<String>,
    },
    Pause {
        target: String,
        duration: Duration,
    },
    CpuLimit {
        target: String,
        percent: u8,
    },
    MemoryLimit {
        target: String,
        limit_mb: usize,
    },
}

/// Network fault types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkFault {
    Latency {
        segment: String,
        ms: u32,
        jitter_ms: u32,
    },
    PacketLoss {
        segment: String,
        percent: f32,
    },
    BandwidthLimit {
        segment: String,
        kbps: u32,
    },
    Partition {
        segment: String,
        direction: PartitionDirection,
    },
    ConnectionReset {
        segment: String,
        after_bytes: Option<u64>,
    },
    Duplicate {
        segment: String,
        percent: f32,
    },
    Reorder {
        segment: String,
        percent: f32,
        gap: u32,
    },
    Corruption {
        segment: String,
        percent: f32,
    },
    SlowSend {
        segment: String,
        bytes_per_sec: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PartitionDirection {
    Bidirectional,
    Inbound,
    Outbound,
}

/// Storage fault types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageFault {
    DiskFull {
        target: String,
        percent: f32,
    },
    ReadOnly {
        target: String,
    },
    CorruptFile {
        target: String,
        file_pattern: String,
        corruption_type: CorruptionType,
    },
    TruncateFile {
        target: String,
        file_pattern: String,
        percent: f32,
    },
    DeleteFile {
        target: String,
        file_pattern: String,
    },
    QuotaExceeded {
        target: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CorruptionType {
    BitFlip {
        probability: f32,
    },
    Truncate {
        percent: f32,
    },
    ZeroOut {
        range_start: f32,
        range_end: f32,
    },
    ReplacePattern {
        pattern: Vec<u8>,
        replacement: Vec<u8>,
    },
}

/// Protocol fault types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProtocolFault {
    MalformedFrames { target: String, rate: u32 },
    InvalidHandshakes { target: String, rate: u32 },
    OversizedMessages { target: String, size_mb: u32 },
    PeerChurn { target: String, rate_per_sec: u32 },
    InvalidPeerAnnouncements { target: String },
}

/// Combined fault specification for scenarios
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FaultSpec {
    pub process: Option<Vec<ProcessFault>>,
    pub network: Option<Vec<NetworkFault>>,
    pub storage: Option<Vec<StorageFault>>,
    pub protocol: Option<Vec<ProtocolFault>>,
    pub duration: Option<Duration>,
}

impl FaultSpec {
    pub fn empty() -> Self {
        Self {
            process: None,
            network: None,
            storage: None,
            protocol: None,
            duration: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn injector() -> FaultInjector {
        FaultInjector::new(LabConfig::default())
    }

    fn assert_not_implemented(r: Result<()>) {
        let e = r.expect_err("an unimplemented fault must not report Ok");
        assert!(e.to_string().contains("not implemented"), "{e}");
    }

    #[tokio::test]
    async fn unimplemented_process_faults_fail() {
        let t = || "validator-1".to_string();
        let fi = injector();
        for f in [
            ProcessFault::Restart {
                target: t(),
                catch_up_from: None,
            },
            ProcessFault::Pause {
                target: t(),
                duration: Duration::from_secs(1),
            },
            ProcessFault::CpuLimit {
                target: t(),
                percent: 50,
            },
            ProcessFault::MemoryLimit {
                target: t(),
                limit_mb: 512,
            },
        ] {
            assert_not_implemented(fi.inject_process_fault(f).await);
        }
    }

    #[tokio::test]
    async fn unimplemented_network_faults_fail() {
        let s = || "S1".to_string();
        let fi = injector();
        for f in [
            NetworkFault::Latency {
                segment: s(),
                ms: 10,
                jitter_ms: 1,
            },
            NetworkFault::PacketLoss {
                segment: s(),
                percent: 0.1,
            },
            NetworkFault::BandwidthLimit {
                segment: s(),
                kbps: 100,
            },
            NetworkFault::ConnectionReset {
                segment: s(),
                after_bytes: None,
            },
            NetworkFault::Duplicate {
                segment: s(),
                percent: 0.1,
            },
            NetworkFault::Reorder {
                segment: s(),
                percent: 0.1,
                gap: 2,
            },
            NetworkFault::Corruption {
                segment: s(),
                percent: 0.1,
            },
            NetworkFault::SlowSend {
                segment: s(),
                bytes_per_sec: 10,
            },
        ] {
            assert_not_implemented(fi.inject_network_fault(f).await);
        }
    }

    #[tokio::test]
    async fn protocol_faults_fail() {
        let t = || "validator-1".to_string();
        let fi = injector();
        for f in [
            ProtocolFault::MalformedFrames {
                target: t(),
                rate: 1,
            },
            ProtocolFault::InvalidHandshakes {
                target: t(),
                rate: 1,
            },
            ProtocolFault::OversizedMessages {
                target: t(),
                size_mb: 1,
            },
            ProtocolFault::PeerChurn {
                target: t(),
                rate_per_sec: 1,
            },
            ProtocolFault::InvalidPeerAnnouncements { target: t() },
        ] {
            assert_not_implemented(fi.inject_protocol_fault(f).await);
        }
    }

    #[tokio::test]
    async fn replace_pattern_corruption_fails() {
        let f = StorageFault::CorruptFile {
            target: "validator-1".to_string(),
            file_pattern: "x".to_string(),
            corruption_type: CorruptionType::ReplacePattern {
                pattern: vec![1],
                replacement: vec![2],
            },
        };
        assert_not_implemented(injector().inject_storage_fault(f).await);
    }

    #[tokio::test]
    async fn unknown_segment_is_an_error() {
        let fi = injector();
        let f = NetworkFault::Partition {
            segment: "S99".to_string(),
            direction: PartitionDirection::Bidirectional,
        };
        let e = fi.inject_network_fault(f).await.unwrap_err();
        assert!(e.to_string().contains("unknown network segment"), "{e}");
        let e = fi.heal_partition("S99").await.unwrap_err();
        assert!(e.to_string().contains("unknown network segment"), "{e}");
    }

    /// A kill that matches no process (only the shell running it) used to
    /// return Ok.
    #[tokio::test]
    async fn kill_with_no_matching_process_fails() {
        let mut config = LabConfig::default();
        config.validators.truncate(1);
        config.validators[0].data_dir = format!("maya2c-no-such-dir-{}", uuid::Uuid::new_v4());
        let f = ProcessFault::Kill {
            target: config.validators[0].name.clone(),
        };
        let e = FaultInjector::new(config)
            .inject_process_fault(f)
            .await
            .unwrap_err();
        assert!(e.to_string().contains("no process matched"), "{e}");
    }

    #[tokio::test]
    async fn kill_unknown_target_fails() {
        let f = ProcessFault::Kill {
            target: "nobody".to_string(),
        };
        assert!(injector().inject_process_fault(f).await.is_err());
    }
}
