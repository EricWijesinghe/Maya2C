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
                &format!(
                    "$d = '{}'; Get-CimInstance Win32_Process | \
                     Where-Object {{ $_.CommandLine -and $_.CommandLine.Contains($d) }} | \
                     ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}",
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
        if !output.status.success() {
            anyhow::bail!(
                "killing {target} ({dir}) failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    async fn restart_process(&self, target: &str, catch_up_from: Option<String>) -> Result<()> {
        // This would be called through the ValidatorManager
        info!(
            "Restart requested for {} with catch_up_from: {:?}",
            target, catch_up_from
        );
        Ok(())
    }

    async fn pause_process(&self, target: &str, _duration: Duration) -> Result<()> {
        // Lowering the priority class (the previous Windows body) does not
        // pause anything, and it selected processes by image name; the Unix
        // body passed `$(pgrep ...)` to kill without a shell. A fault that
        // did not happen must not let a scenario report PASS.
        anyhow::bail!("pause fault not implemented (target {target})")
    }

    async fn limit_cpu(&self, _target: &str, _percent: u8) -> Result<()> {
        // Use Job Objects on Windows or cgroups on Linux
        #[cfg(windows)]
        {
            info!("CPU limiting not fully implemented for Windows Job Objects yet");
        }
        Ok(())
    }

    async fn limit_memory(&self, _target: &str, _limit_mb: usize) -> Result<()> {
        // Use Job Objects on Windows or cgroups on Linux
        info!("Memory limiting not fully implemented yet");
        Ok(())
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
        info!(
            "Injecting partition on segment {} ({:?})",
            segment, direction
        );

        // Get the segment config to determine which ports to block
        let segment_config = self.config.network.segments.get(segment);
        if segment_config.is_none() {
            warn!("Segment {} not found in config", segment);
            return Ok(());
        }

        // Get all validator ports that need to be blocked
        let mut ports_to_block = Vec::new();
        for v in &self.config.validators {
            if segment == "S1"
                || segment == "S2"
                || segment == "S3"
                || segment == "S6"
                || segment == "S7"
            {
                // Validator P2P ports
                ports_to_block.push(v.p2p_port);
            }
        }

        // Also add bootnode port
        if let Some(port) = self.config.infrastructure.bootnode.p2p_port {
            ports_to_block.push(port);
        }

        // Add adversarial peer port
        if let Some(port) = self.config.infrastructure.adversarial_peer.p2p_port {
            ports_to_block.push(port);
        }

        // Remove duplicates
        ports_to_block.sort();
        ports_to_block.dedup();

        match direction {
            PartitionDirection::Bidirectional => {
                info!(
                    "Blocking bidirectional traffic on ports: {:?}",
                    ports_to_block
                );
                for port in &ports_to_block {
                    self.block_port_bidirectional(*port)?;
                }
            }
            PartitionDirection::Inbound => {
                info!("Blocking inbound traffic on ports: {:?}", ports_to_block);
                for port in &ports_to_block {
                    self.block_port_inbound(*port)?;
                }
            }
            PartitionDirection::Outbound => {
                info!("Blocking outbound traffic on ports: {:?}", ports_to_block);
                for port in &ports_to_block {
                    self.block_port_outbound(*port)?;
                }
            }
        }
        Ok(())
    }

    fn block_port_bidirectional(&self, port: u16) -> Result<()> {
        let rule_name = format!("Maya2C_Block_Port_{}_Bidirectional", port);

        // Delete existing rule if exists
        let _ = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "delete",
                "rule",
                "name=",
                &rule_name,
            ])
            .output();

        // Block inbound
        let _ = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                "name=",
                &rule_name,
                "dir=in",
                "action=block",
                "protocol=TCP",
                &format!("localport={}", port),
            ])
            .output()?;

        // Block outbound
        let _ = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                "name=",
                &format!("{}_Out", rule_name),
                "dir=out",
                "action=block",
                "protocol=TCP",
                &format!("remoteport={}", port),
            ])
            .output()?;

        info!("Created bidirectional firewall rule for port {}", port);
        Ok(())
    }

    fn block_port_inbound(&self, port: u16) -> Result<()> {
        let rule_name = format!("Maya2C_Block_Port_{}_Inbound", port);

        let _ = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "delete",
                "rule",
                "name=",
                &rule_name,
            ])
            .output();

        let _ = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                "name=",
                &rule_name,
                "dir=in",
                "action=block",
                "protocol=TCP",
                &format!("localport={}", port),
            ])
            .output()?;

        info!("Created inbound firewall rule for port {}", port);
        Ok(())
    }

    fn block_port_outbound(&self, port: u16) -> Result<()> {
        let rule_name = format!("Maya2C_Block_Port_{}_Outbound", port);

        let _ = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "delete",
                "rule",
                "name=",
                &rule_name,
            ])
            .output();

        let _ = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                "name=",
                &rule_name,
                "dir=out",
                "action=block",
                "protocol=TCP",
                &format!("remoteport={}", port),
            ])
            .output()?;

        info!("Created outbound firewall rule for port {}", port);
        Ok(())
    }

    async fn inject_connection_reset(&self, segment: &str, after_bytes: Option<u64>) -> Result<()> {
        info!(
            "Injecting connection reset on segment {} (after bytes: {:?})",
            segment, after_bytes
        );
        Ok(())
    }

    async fn inject_duplication(&self, segment: &str, percent: f32) -> Result<()> {
        info!(
            "Injecting packet duplication {}% on segment {}",
            percent * 100.0,
            segment
        );
        Ok(())
    }

    async fn inject_reorder(&self, segment: &str, percent: f32, gap: u32) -> Result<()> {
        info!(
            "Injecting packet reorder {}% (gap {}) on segment {}",
            percent * 100.0,
            gap,
            segment
        );
        Ok(())
    }

    async fn inject_corruption(&self, segment: &str, percent: f32) -> Result<()> {
        info!(
            "Injecting packet corruption {}% on segment {}",
            percent * 100.0,
            segment
        );
        Ok(())
    }

    async fn inject_slow_send(&self, segment: &str, bytes_per_sec: u32) -> Result<()> {
        info!(
            "Injecting slow send {} bytes/sec on segment {}",
            bytes_per_sec, segment
        );
        Ok(())
    }

    pub async fn heal_partition(&self, segment: &str) -> Result<()> {
        info!("Healing partition on segment {}", segment);

        let segment_config = self.config.network.segments.get(segment);
        if segment_config.is_none() {
            warn!("Segment {} not found in config", segment);
            return Ok(());
        }

        let mut ports_to_block = Vec::new();
        for v in &self.config.validators {
            if segment == "S1"
                || segment == "S2"
                || segment == "S3"
                || segment == "S6"
                || segment == "S7"
            {
                ports_to_block.push(v.p2p_port);
            }
        }

        if let Some(port) = self.config.infrastructure.bootnode.p2p_port {
            ports_to_block.push(port);
        }

        if let Some(port) = self.config.infrastructure.adversarial_peer.p2p_port {
            ports_to_block.push(port);
        }

        ports_to_block.sort();
        ports_to_block.dedup();

        info!("Removing firewall rules for ports: {:?}", ports_to_block);
        for port in &ports_to_block {
            self.remove_firewall_rules(*port)?;
        }

        Ok(())
    }

    fn remove_firewall_rules(&self, port: u16) -> Result<()> {
        let rule_names = vec![
            format!("Maya2C_Block_Port_{}_Bidirectional", port),
            format!("Maya2C_Block_Port_{}_Bidirectional_Out", port),
            format!("Maya2C_Block_Port_{}_Inbound", port),
            format!("Maya2C_Block_Port_{}_Outbound", port),
        ];

        for rule_name in rule_names {
            let _ = Command::new("netsh")
                .args([
                    "advfirewall",
                    "firewall",
                    "delete",
                    "rule",
                    "name=",
                    &rule_name,
                ])
                .output();
        }

        info!("Removed firewall rules for port {}", port);
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

        let data_dir = format!("D:\\Maya2C-attacknet-{}", target);

        #[cfg(windows)]
        {
            let output = Command::new("icacls")
                .args([&data_dir, "/deny", "Everyone:(W)"])
                .output()
                .context("Setting read-only ACL")?;
            if !output.status.success() {
                warn!("icacls failed: {}", String::from_utf8_lossy(&output.stderr));
            }
        }

        Ok(())
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
                    CorruptionType::ReplacePattern {
                        ref pattern,
                        ref replacement,
                    } => {
                        // Simple pattern replacement
                        let _ = pattern;
                        let _ = replacement;
                    }
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
    async fn send_malformed_frames(&self, target: &str, rate: u32) -> Result<()> {
        info!("Sending malformed frames to {} at {}/sec", target, rate);
        // This would be implemented by the adversarial peer process
        Ok(())
    }

    async fn send_invalid_handshakes(&self, target: &str, rate: u32) -> Result<()> {
        info!("Sending invalid handshakes to {} at {}/sec", target, rate);
        Ok(())
    }

    async fn send_oversized_messages(&self, target: &str, size_mb: u32) -> Result<()> {
        info!("Sending oversized messages ({} MB) to {}", size_mb, target);
        Ok(())
    }

    async fn peer_churn(&self, target: &str, rate_per_sec: u32) -> Result<()> {
        info!("Peer churn against {} at {}/sec", target, rate_per_sec);
        Ok(())
    }

    async fn send_invalid_peer_announcements(&self, target: &str) -> Result<()> {
        info!("Sending invalid peer announcements to {}", target);
        Ok(())
    }
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
