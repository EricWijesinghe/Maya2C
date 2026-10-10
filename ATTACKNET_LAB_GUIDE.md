# Maya2C Attacknet Lab - Quick Start Guide

## Overview

The attacknet lab simulates a **four-validator isolated virtual infrastructure** on a single physical workstation (NITROZEUS). It provides:

- **4 validators** with unique keys, data dirs, and network ports
- **1 bootnode** for peer discovery
- **1 RPC gateway** for aggregated access
- **1 adversarial peer** for protocol-level attacks
- **Monitoring stack** (Prometheus, Grafana, Alertmanager, Vector)
- **Automated scenario runner** with 73 defined attack scenarios

## Hardware Requirements

| Resource | Available | Allocated to Lab |
|----------|-----------|------------------|
| CPU | 24 cores (Intel Core Ultra 9 275HX) | 18 cores (4 validators × 3 + infra) |
| RAM | 32 GB | ~12 GB (1.5 GB × 4 validators + infra) |
| Disk D: | 259 GB free | < 25 GB |
| Commit Charge | 48.5 GB max | ~26 GB peak |

## Quick Start

### 1. Prerequisites

```powershell
# Verify Rust toolchain
rustc --version  # Should be nightly-2026-07-15

# Check commit charge
$freeGB = [math]::Round((Get-CimInstance Win32_OperatingSystem).FreeVirtualMemory / 1MB, 1)
$freeGB  # Should be > 3.5 GB
```

### 2. Build the Lab

```powershell
cd D:\Maya2C

# Build all required binaries
cargo build --release -p maya2c-node -p maya-api-gateway -p attacknet-controller

# Build attacknet adversary (if implemented)
cargo build --release -p attacknet-adversary 2>&1 || Write-Warning "Adversary not yet implemented"
```

### 3. Provision the Lab

```powershell
# Option A: Use the PowerShell script
.\scripts\attacknet-lab.ps1 -Action provision

# Option B: Use the Rust controller
cargo run -p attacknet-controller -- provision --config D:\Maya2C-attacknet-control\lab.toml
```

This creates:
- `D:\Maya2C-attacknet-v1..4` - Validator data directories
- `D:\Maya2C-attacknet-bootnode` - Bootnode directory
- `D:\Maya2C-attacknet-adversary` - Adversarial peer directory
- `D:\Maya2C-attacknet-genesis` - Shared genesis file
- `D:\Maya2C-attacknet-control` - Control directory with configs
- Unique validator keys in each data directory

### 4. Start the Lab

```powershell
# Option A: PowerShell script
.\scripts\attacknet-lab.ps1 -Action start

# Option B: Rust controller
cargo run -p attacknet-controller -- start --config D:\Maya2C-attacknet-control\lab.toml
```

Expected startup sequence:
1. Bootnode starts on port 33310
2. 4 validators start on ports 33300-33303 (P2P), 34300-34303 (RPC), 35300-35303 (metrics)
3. Consensus forms (3+ blocks)
4. RPC gateway starts on port 34310
5. Adversarial peer starts on port 33320
6. Monitoring stack starts in WSL2 Docker

### 5. Verify Lab Status

```powershell
.\scripts\attacknet-lab.ps1 -Action status

# Or via Rust controller
cargo run -p attacknet-controller -- status
```

### 6. Run Attack Scenarios

```powershell
# Run a specific scenario
.\scripts\attacknet-lab.ps1 -Action scenario -ScenarioId "1.1" -Rounds 3

# Or via Rust controller
cargo run -p attacknet-controller -- run "1.1" --rounds 3
```

### 7. Stop the Lab

```powershell
.\scripts\attacknet-lab.ps1 -Action stop

# Emergency termination
.\scripts\attacknet-lab.ps1 -Action down
```

## Lab Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  HOST: NITROZEUS (Windows 11)                                   │
│  24 cores │ 32 GB RAM │ 259 GB free on D:                      │
└─────────────────────────────────────────────────────────────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        ▼                     ▼                     ▼
┌───────────────┐     ┌───────────────┐     ┌───────────────┐
│  VALIDATOR 1  │     │  VALIDATOR 2  │     │  VALIDATOR 3  │
│  P2P: 33300   │◄───►│  P2P: 33301   │◄───►│  P2P: 33302   │
│  RPC: 34300   │     │  RPC: 34301   │     │  RPC: 34302   │
│  Metrics:35300│     │  Metrics:35301│     │  Metrics:35302│
└───────┬───────┘     └───────┬───────┘     └───────┬───────┘
        │                     │                     │
        └─────────────────────┼─────────────────────┘
                              │
                    ┌─────────┴─────────┐
                    │  VALIDATOR 4      │
                    │  P2P: 33303       │
                    │  RPC: 34303       │
                    │  Metrics: 35303   │
                    └───────────────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        ▼                     ▼                     ▼
┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐
│    BOOTNODE      │  │   RPC GATEWAY    │  │ ADVERSARIAL PEER │
│    P2P: 33310    │  │   REST: 34310    │  │   P2P: 33320     │
│    RPC: 34310    │  │   WS: 34311      │  │   RPC: 34320     │
└──────────────────┘  └──────────────────┘  └──────────────────┘
        │                     │                     │
        └─────────────────────┼─────────────────────┘
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│  MONITORING STACK (WSL2 Docker)                                 │
│  Prometheus:35310  Grafana:35311  Alertmanager:35312           │
│  Vector:35313  (logs)                                          │
└─────────────────────────────────────────────────────────────────┘
```

## Network Ports

| Range | Purpose |
|-------|---------|
| 33300-33303 | Validator P2P (libp2p) |
| 33310 | Bootnode P2P |
| 33320 | Adversarial Peer P2P |
| 34300-34303 | Validator JSON-RPC |
| 34310 | RPC Gateway (REST) |
| 34311 | RPC Gateway (WebSocket) |
| 34320 | Adversarial Peer RPC |
| 35300-35303 | Validator Metrics |
| 35310 | Prometheus |
| 35311 | Grafana |
| 35312 | Alertmanager |
| 35313 | Vector (log ingest) |

## Scenario Categories

| Category | Name | Scenarios | Priority |
|----------|------|-----------|----------|
| 1 | Consensus Adversary | 15 | CRITICAL |
| 2 | Network Adversary | 18 | HIGH |
| 3 | Transaction Adversary | 16 | HIGH |
| 4 | API Adversary | 16 | HIGH |
| 5 | VM/Contract Adversary | 13 | MEDIUM |
| 6 | Storage Adversary | 13 | HIGH |
| 7 | Operational Adversary | 10 | MEDIUM |
| 8 | Test-Key Compromise | 7 | CRITICAL |

**Total: 108 scenarios**

### Key Scenarios Implemented

| ID | Name | Description | Status |
|----|------|-------------|--------|
| 1.1 | validator_silence | One validator stops signing | ✅ Implemented |
| 1.8 | quorum_edge_conditions | f/f+1 validators down | ✅ Implemented |
| 2.3 | random_bytes | TCP garbage on P2P ports | ✅ Implemented |
| 8.1 | copied_validator_key | Twin process with same key | ✅ Implemented |

### Running the 4-Week Schedule

```powershell
# Week 1: Consensus + Network
cargo run -p attacknet-controller -- run-category 1 --rounds 1
cargo run -p attacknet-controller -- run-category 2 --rounds 1

# Week 2: Transaction + API
cargo run -p attacknet-controller -- run-category 3 --rounds 1
cargo run -p attacknet-controller -- run-category 4 --rounds 1

# Week 3: Combined stress
cargo run -p attacknet-controller -- run-schedule --week 3

# Week 4: Full combined + key compromise
cargo run -p attacknet-controller -- run-schedule --week 4
```

## Evidence Collection

Each scenario run produces evidence in:
```
D:\Maya2C\reports\attacknet\runs\<date>\<scenario-id>\
├── evidence.json      # Structured evidence (machine-readable)
├── summary.md         # Human-readable summary
├── validator logs     # Referenced from evidence
└── packet captures    # If enabled
```

## Monitoring

- **Prometheus**: http://127.0.0.1:35310
- **Grafana**: http://127.0.0.1:35311 (admin/lab)
- **Alertmanager**: http://127.0.0.1:35312
- **Vector logs**: http://127.0.0.1:35313

## Safety Features

The lab enforces strict authorization boundaries:

1. **Chain ID verification**: All operations check `MAYA2C_CHAIN_ID=maya2c-attacknet-lab`
2. **Data directory validation**: All paths must be under `D:\Maya2C-attacknet-v*`
3. **Production key detection**: Scans for "prod", "mainnet", "real" in lab directories
4. **Resource limits**: Emergency shutdown at 42 GB commit charge
5. **Destructive operation guards**: Require explicit lab identifier

## Troubleshooting

### Low Commit Charge
```powershell
# Pause live testnet peers
Get-CimInstance Win32_Process -Filter "Name = 'maya2c-peer.exe'" |
    Where-Object { $_.CommandLine -match 'peer[78]\\data' } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
```

### Validator Won't Start
```powershell
# Check logs
Get-Content D:\Maya2C-attacknet-v1\node.log -Tail 50

# Verify key exists
Test-Path D:\Maya2C-attacknet-v1\validator.key

# Check port availability
netstat -an | findstr "33300 34300 35300"
```

### Consensus Not Forming
```powershell
# Check peer connections
curl http://127.0.0.1:34300/get_peers

# Verify bootnode is running
curl http://127.0.0.1:34310/health
```

### Monitoring Stack Not Starting
```powershell
# Check WSL2 Docker
wsl -- docker version

# Manual start
cd D:\Maya2C
wsl -- docker-compose -f docker-compose.monitoring.yml up -d
```

## Reports

Daily reports are generated at:
```
D:\Maya2C\reports\attacknet\<date>.md
D:\Maya2C\reports\attacknet\runs\<date>\<scenario>\
```

Aggregate reports:
```powershell
cargo run -p attacknet-controller -- report --date 2026-10-09
```

## Files in This Lab

| File | Purpose |
|------|---------|
| `docker-compose.monitoring.yml` | Monitoring stack (Prometheus, Grafana, Alertmanager, Vector) |
| `prometheus.yml` | Prometheus scrape config |
| `grafana-datasources.yml` | Grafana datasource config |
| `alertmanager.yml` | Alertmanager config |
| `vector.yml` | Vector log aggregation config |
| `scripts/attacknet-lab.ps1` | PowerShell lab orchestration |
| `bins/attacknet-controller/` | Rust lab controller binary |
| `reports/attacknet/` | Evidence and reports |

## Authorization Boundary

**ALL TESTING TARGETS ONLY:**
- ✅ Maya2C services running locally
- ✅ Virtual machines created for this laboratory
- ✅ Containers/network namespaces created for this laboratory
- ✅ Explicitly authorized Maya2C test infrastructure
- ✅ Test-only copies of services and data

**NEVER TARGET:**
- ❌ Unrelated public hosts
- ❌ Third-party infrastructure
- ❌ GitHub, cloud APIs, external services
- ❌ Production systems
- ❌ Systems without explicit authorization

---

*Lab ID: maya2c-attacknet-lab-v1*
*Chain ID: maya2c-attacknet-lab*
*Generated: 2026-10-09*