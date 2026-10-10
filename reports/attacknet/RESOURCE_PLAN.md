# Resource Allocation Plan

**Date:** 2026-10-09
**Lab ID:** `maya2c-attacknet-lab-v1`
**Chain ID:** `maya2c-attacknet-lab`

## Host Reservations (Non-negotiable)

| Resource | Reserved | Rationale |
|---|---|---|
| **CPU** | 6 cores (25%) | Host OS, Claude, IDE, shell, git, emergency teardown |
| **RAM (Physical)** | 4 GB | Host working set + page file buffer |
| **Commit Charge** | 8 GB | Page file headroom; prevents OOM kills |
| **Disk D:** | 50 GB | OS updates, temp, artifact staging |
| **Disk C:** | 20 GB | Page file growth, Windows updates |
| **File Descriptors** | 10,000 | System + monitoring tools |
| **Network** | Unrestricted | Host needs full connectivity |

## Validator Allocations (×4)

| Resource | Per Validator | Total (4) | Enforcement |
|---|---|---|---|
| **CPU** | 3 cores (affinity) | 12 cores | `Start-Process -ProcessorAffinity` / `taskset` in WSL |
| **RAM (Working Set)** | 1.5 GB max | 6 GB | Windows Job Object `ProcessMemoryLimit` |
| **Commit Charge** | 3 GB max | 12 GB | Job Object `JobMemoryLimit` |
| **Disk (Data Dir)** | 5 GB quota | 20 GB | NTFS quota on `D:\Maya2C-attacknet-v{N}` |
| **Disk (Logs)** | 500 MB | 2 GB | Log rotation (10 × 50 MB) |
| **P2P Port** | 33300–33303 | 4 ports | Explicit config |
| **RPC Port** | 34300–34303 | 4 ports | Explicit config |
| **Metrics Port** | 35300–35303 | 4 ports | Prometheus scrape |
| **File Descriptors** | 2,000 | 8,000 | Job Object `ActiveProcessLimit` |

## Supporting Services

| Service | CPU | RAM | Disk | Ports | Notes |
|---|---|---|---|---|---|
| **Bootnode** | 0.5 core | 256 MB | 100 MB | 33310 | Static peer ID; no validator key |
| **RPC Gateway** | 1 core | 512 MB | 200 MB | 34310 | Rate-limited; auth optional |
| **Explorer/Indexer** | 1 core | 1 GB | 2 GB | 34311 | Subscribes to all validators |
| **Monitoring Stack** | 2 cores | 2 GB | 5 GB | 35310–35314 | Prometheus + Grafana + Alertmanager |
| **Traffic Generator** | 2 cores | 1 GB | 100 MB | Ephemeral | Controlled by test controller |
| **Adversarial Peer** | 1 core | 512 MB | 100 MB | 33320 | Isolated; no validator keys |
| **Test Controller** | 0.5 core | 256 MB | 100 MB | — | Orchestrates scenarios |
| **Log/Metrics Collector** | 0.5 core | 512 MB | 2 GB | — | Fluent Bit / Vector |

**Supporting Total:** ~8.5 cores, ~6 GB RAM, ~10 GB disk

## Aggregate Allocation

| Category | CPU Cores | RAM (GB) | Commit (GB) | Disk (GB) |
|---|---|---|---|---|
| Host Reserve | 6 | 4 | 8 | 50 |
| 4 Validators | 12 | 6 | 12 | 22 |
| Supporting | 8.5 | 6 | 6 | 10 |
| **Total** | **26.5** | **16** | **26** | **82** |
| **Available** | **24** | **32** | **48.5** | **259** |
| **Headroom** | **-2.5** | **16** | **22.5** | **177** |

**CPU overcommit by 2.5 cores** — acceptable because:
- Validators are I/O-bound (RocksDB, network)
- Supporting services are idle during baseline
- Load generator only active during stress stages
- Affinity prevents cache thrashing

## Storage Growth Limits

| Path | Soft Limit | Hard Limit | Action at Soft | Action at Hard |
|---|---|---|---|---|
| `D:\Maya2C-attacknet-v*/data` | 4 GB | 5 GB | Alert; pause ingest | Kill validator; investigate |
| `D:\Maya2C-attacknet-v*/logs` | 400 MB | 500 MB | Rotate aggressively | Truncate oldest |
| `D:\Maya2C-attacknet-reports` | 1 GB | 2 GB | Compress old runs | Delete oldest |
| `D:\Maya2C-attacknet-monitoring` | 4 GB | 5 GB | Drop resolution | Pause scrape |

## Log Retention

| Log Type | Retention | Compression | Max Files |
|---|---|---|---|
| Validator stdout/stderr | 7 days | gzip after 1 day | 10 |
| Consensus traces | 3 days | gzip after 6 hours | 20 |
| RPC access logs | 1 day | gzip after 1 hour | 50 |
| Attack artifacts | 30 days | gzip immediately | 100 |
| Metrics (Prometheus) | 14 days | WAL compaction | — |

## Emergency Shutdown Thresholds

| Metric | Threshold | Action |
|---|---|---|
| **Host commit charge** | > 42 GB (87%) | `cargo xtask down`; kill all lab processes |
| **Host physical RAM** | < 1 GB free | Pause load gen; kill adversarial peer |
| **D: free space** | < 20 GB | Stop validators; compress logs |
| **Validator CPU** | > 90% for 5 min | Throttle via Job Object |
| **Validator RAM** | > 1.8 GB working set | Restart validator (catch-up) |
| **Validator disk** | > 4.5 GB | Pause block processing; compact DB |
| **Log collection lag** | > 10 min behind | Drop debug logs; keep error/warn |
| **Scenario duration** | > 4 hours | Auto-terminate; mark incomplete |

## Automatic Termination Conditions

A running scenario **must terminate** when ANY of:

1. Host commit charge > 42 GB
2. D: free space < 15 GB
3. Any validator process crashes 3× in 10 minutes
4. Consensus fork detected (divergent state roots)
5. Scenario wall-clock > 4 hours (configurable per stage)
6. `STOP_ATTACKNET` file appears in `D:\Maya2C-attacknet-control\`
7. Ctrl+C / SIGINT received by test controller

## Resource Enforcement Implementation

### Windows (Host Validators)

```powershell
# Create Job Object for each validator
$job = New-Object System.Diagnostics.Process
$job.StartInfo.FileName = "maya2c-node.exe"
$job.StartInfo.Arguments = "--config ... --data-dir D:\Maya2C-attacknet-v1 ..."
$job.StartInfo.ProcessorAffinity = 0b111  # Cores 0-2 for v1, etc.
# Memory limits via Job Objects (requires P/Invoke or third-party)
# Practical: monitor via Get-Process and restart if exceeded
```

### WSL2 (Alternative for Network Isolation)

```bash
# Run each validator in separate cgroup
systemd-run --scope --user -p CPUQuota=12% -p MemoryMax=1.5G \
  -p MemorySwapMax=0 -p TasksMax=2000 \
  maya2c-node --config ... --data-dir /mnt/d/Maya2C-attacknet-v1 ...
```

**Decision:** Use **Windows host processes** with PowerShell Job Objects for CPU affinity and monitoring. Network isolation via distinct loopback ports. WSL2 reserved for eBPF/XDP adversarial tests only.

## Lab Identifier Enforcement

Every destructive command checks:

```rust
fn verify_lab_context() -> Result<()> {
    let chain_id = std::env::var("MAYA2C_CHAIN_ID")?;
    if chain_id != "maya2c-attacknet-lab" {
        return Err("Refusing: CHAIN_ID mismatch");
    }
    let data_dir = std::env::var("MAYA2C_DATA_DIR")?;
    if !data_dir.starts_with(r"D:\Maya2C-attacknet-v") {
        return Err("Refusing: data dir outside lab");
    }
    // Check for production keys
    if std::fs::read_dir(&data_dir)?.any(|e| e?.file_name().to_string_lossy().contains("prod")) {
        return Err("Refusing: production key detected");
    }
    Ok(())
}
```

All lab binaries and scripts **must** call this before destructive operations.