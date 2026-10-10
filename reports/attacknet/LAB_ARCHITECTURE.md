# Lab Architecture

**Lab ID:** `maya2c-attacknet-lab-v1`
**Date:** 2026-10-09
**Chain ID:** `maya2c-attacknet-lab`

## Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                        HOST: NITROZEUS (Windows 11)                         │
│  24 cores │ 32 GB RAM │ 259 GB free on D: │ Hyper-V present, firmware off  │
└─────────────────────────────────────────────────────────────────────────────┘
                                              │
        ┌─────────────────────────────────────┼─────────────────────────────────────┐
        │                                     │                                     │
        ▼                                     ▼                                     ▼
┌───────────────┐                   ┌───────────────┐                   ┌───────────────┐
│  VALIDATOR 1  │                   │  VALIDATOR 2  │                   │  VALIDATOR 3  │
│  (Process)    │                   │  (Process)    │                   │  (Process)    │
│               │                   │               │                   │               │
│ RPC: 34300    │                   │ RPC: 34301    │                   │ RPC: 34302    │
│ P2P: 33300    │◄──────libp2p──────►│ P2P: 33301    │◄──────libp2p──────►│ P2P: 33302    │
│ Metrics:35300 │                   │ Metrics:35301 │                   │ Metrics:35302 │
│ Data: v1/     │                   │ Data: v2/     │                   │ Data: v3/     │
│ CPU: cores 0-2│                   │ CPU: cores 3-5│                   │ CPU: cores 6-8│
│ Mem: ≤1.5 GB  │                   │ Mem: ≤1.5 GB  │                   │ Mem: ≤1.5 GB  │
└───────┬───────┘                   └───────┬───────┘                   └───────┬───────┘
        │                                     │                                     │
        └─────────────────────────────────────┼─────────────────────────────────────┘
                                              │
                                              ▼
        ┌─────────────────────────────────────────────────────────────────────┐
        │                    VALIDATOR 4 (Process)                            │
        │  RPC: 34303  │  P2P: 33303  │  Metrics: 35303  │  CPU: cores 9-11  │
        │  Data: v4/   │  Mem: ≤1.5 GB                                      │
        └─────────────────────────────────────────────────────────────────────┘
                                              │
        ┌─────────────────────────────────────┼─────────────────────────────────────┐
        │                                     │                                     │
        ▼                                     ▼                                     ▼
┌─────────────────────┐             ┌─────────────────────┐             ┌─────────────────────┐
│    BOOTNODE         │             │   RPC GATEWAY       │             │  ADVERSARIAL PEER   │
│  (Process)          │             │  (maya-api-gateway) │             │  (Process)          │
│  P2P: 33310         │             │  REST: 34310        │             │  P2P: 33320         │
│  No validator key   │             │  WS: 34311          │             │  No validator key   │
│  Static peer ID     │             │  Rate: 1000/s       │             │  Malicious traffic  │
└─────────────────────┘             └─────────────────────┘             └─────────────────────┘
        │                                     │                                     │
        ▼                                     ▼                                     ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                    MONITORING STACK (WSL2 Containers)                       │
│  ┌─────────┐  ┌─────────┐  ┌─────────────┐  ┌─────────────────────────┐   │
│  │Prometheus│  │ Grafana │  │ Alertmanager│  │   Log Aggregator        │   │
│  │ 35310    │  │ 35311   │  │  35312      │  │   (Vector/Fluent Bit)   │   │
│  └─────────┘  └─────────┘  └─────────────┘  └─────────────────────────┘   │
│  Scrapes: 35300-35303, 34310, 33310, 33320                                │
└─────────────────────────────────────────────────────────────────────────────┘
        │
        ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                  TEST CONTROLLER & EVIDENCE COLLECTOR                       │
│  • Orchestrates scenarios (PowerShell / Rust)                               │
│  • Injects faults (network, process, storage)                               │
│  • Collects logs, metrics, packet captures                                  │
│  • Writes reports to D:\Maya2C\reports\attacknet\runs\<date>\<scenario>/   │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Component Details

### Validators (4× Windows Processes)

| Attribute | Value |
|---|---|
| **Binary** | `maya2c-node.exe` (built with `--features production`) |
| **Genesis** | Shared `genesis.json` at `D:\Maya2C-attacknet-genesis\genesis.json` |
| **Keys** | Unique per validator: `validator.key` in each data dir |
| **Identity** | Unique peer ID derived from validator key |
| **Consensus** | DAG-BFT (ADR-027); stake-weighted (ADR-040) |
| **Storage** | RocksDB per validator; `block_cache_mib = 32` |
| **Network** | libp2p over TCP/loopback; Noise XX + ML-KEM-768 handshake |
| **RPC** | JSON-RPC 2.0; rate limited 1000/s per client |
| **Metrics** | Prometheus exposition format on `/metrics` |

### Bootnode (1× Windows Process)

- Same binary, `--bootnode` mode (no validator key)
- Static peer ID published in all validator configs
- Runs on port 33310
- No consensus participation

### RPC Gateway (1× maya-api-gateway Process)

- Aggregates RPC across validators
- Rate limiting, auth, request routing
- WebSocket support for subscriptions
- Metrics on 35310

### Adversarial Peer (1× Custom Process)

- **No validator keys** — never participates in consensus
- Generates malformed frames, garbage, floods
- Controlled by test controller via IPC
- Isolated network namespace (WSL2) when injecting wire-level faults

### Monitoring Stack (WSL2 Docker Compose)

```yaml
# docker-compose.monitoring.yml
services:
  prometheus:
    image: prom/prometheus:v2.54.0
    ports: ["35310:9090"]
    volumes: ["./prometheus.yml:/etc/prometheus/prometheus.yml"]
    command: ["--storage.tsdb.retention.time=14d"]
  grafana:
    image: grafana/grafana:11.1.0
    ports: ["35311:3000"]
    environment: [GF_SECURITY_ADMIN_USER=admin, GF_SECURITY_ADMIN_PASSWORD=lab]
    volumes: ["./dashboards:/etc/grafana/provisioning/dashboards"]
  alertmanager:
    image: prom/alertmanager:v0.27.0
    ports: ["35312:9093"]
  vector:
    image: timberio/vector:0.42.0-alpine
    volumes: ["./vector.yml:/etc/vector/vector.yml"]
    ports: ["35313:8686"]
```

### Test Controller (Rust Binary)

- Single binary: `attacknet-controller` (to be built)
- Commands validators via RPC + process control
- Injects faults via:
  - Process: `Stop-Process` / `Start-Process`
  - Network: `tc` in WSL2 namespaces, `pktmon` filters
  - Storage: NTFS quota, simulated corruption via file replacement
- Records all actions with timestamps to evidence log

## Data Flow

```
┌──────────────┐     Genesis      ┌──────────────┐
│   Ceremony   │ ───────────────► │  genesis.json│
│  (one-time)  │                  │  (shared RO) │
└──────────────┘                  └──────┬───────┘
                                         │
              ┌──────────────────────────┼──────────────────────────┐
              │                          │                          │
              ▼                          ▼                          ▼
       ┌────────────┐             ┌────────────┐             ┌────────────┐
       │ Validator 1│             │ Validator 2│             │ Validator 3│
       │  (v1/)     │             │  (v2/)     │             │  (v3/)     │
       └─────┬──────┘             └─────┬──────┘             └─────┬──────┘
             │                          │                          │
             └──────────────────────────┼──────────────────────────┘
                                        │ libp2p gossip (blocks, votes, attestations)
                                        ▼
                               ┌────────────┐
                               │ Validator 4│
                               │  (v4/)     │
                               └────────────┘
                                        │
              ┌─────────────────────────┼─────────────────────────┐
              │                         │                         │
              ▼                         ▼                         ▼
       ┌────────────┐           ┌────────────┐           ┌────────────┐
       │  Metrics   │           │   Logs     │           │   RPC      │
       │  (push)    │           │  (file)    │           │  (pull)    │
       └─────┬──────┘           └─────┬──────┘           └─────┬──────┘
             │                        │                        │
             └────────────────────────┼────────────────────────┘
                                      ▼
                          ┌─────────────────────┐
                          │  Monitoring Stack   │
                          │  (Prometheus, etc.) │
                          └──────────┬──────────┘
                                     │
                                     ▼
                          ┌─────────────────────┐
                          │  Evidence Collector │
                          │  (structured JSON)  │
                          └─────────────────────┘
```

## Isolation Boundaries

| Boundary | Mechanism | Verification |
|---|---|---|
| **Process** | Separate PID, Job Object | `Get-Process` shows distinct PIDs |
| **Memory** | Job Object `ProcessMemoryLimit` | Monitor working set; restart on breach |
| **CPU** | `ProcessorAffinity` mask | `Get-Process -Id $pid | Select ProcessorAffinity` |
| **Disk** | Separate `D:\Maya2C-attacknet-v{N}\` + NTFS quota | `fsutil quota query` |
| **Network** | Distinct loopback ports | `netstat -an | findstr 3330[0-3]` |
| **Keys** | Unique `validator.key` per data dir | `sha256sum v*/validator.key` all differ |
| **Config** | Per-validator `config.toml` | Diff shows only port/paths differ |
| **Logs** | Per-validator `node.log` + rotation | No cross-contamination |
| **Metrics** | Separate `/metrics` endpoints | Prometheus scrapes 4 targets |

## Network Topology (Logical)

```
                    ┌──────────────┐
                    │   Bootnode   │
                    │  (33310)     │
                    └──────┬───────┘
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
        ▼                  ▼                  ▼
   ┌─────────┐        ┌─────────┐        ┌─────────┐
   │ Val 1   │        │ Val 2   │        │ Val 3   │
   │ (33300) │        │ (33301) │        │ (33302) │
   └────┬────┘        └────┬────┘        └────┬────┘
        │                  │                  │
        └──────────────────┼──────────────────┘
                           │
                           ▼
                    ┌──────────────┐
                    │   Val 4      │
                    │  (33303)     │
                    └──────────────┘

All validators: full mesh via bootnode discovery
Adversarial peer: connects to all, sends garbage
RPC Gateway: connects to all validators' RPC ports
Monitoring: scrapes all metrics ports
```

## Fault Injection Points

| Layer | Injection Method | Controlled By |
|---|---|---|
| **Process** | Kill/start via Job Object | Test Controller (PowerShell) |
| **Network (L4)** | `tc netem` in WSL2 netns | Test Controller (WSL SSH) |
| **Network (L7)** | Custom libp2p proxy (Rust) | Test Controller (IPC) |
| **Storage** | NTFS quota + file replace | Test Controller (PowerShell) |
| **Time** | Virtual clock (simulation only) | `maya-sim` crate |
| **Crypto** | Stolen key twin process | Test Controller (spawn twin) |

## Security Boundaries

```
┌─────────────────────────────────────────────────────────────────┐
│                    AUTHORIZATION BOUNDARY                       │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  ALLOWED TARGETS                                        │   │
│  │  • maya2c-node processes (lab chain ID)                 │   │
│  │  • WSL2 namespaces created for lab                      │   │
│  │  • Local loopback interfaces (127.0.0.1/::1)            │   │
│  │  • Test-only keys in D:\Maya2C-attacknet-v*/            │   │
│  └─────────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  FORBIDDEN TARGETS                                      │   │
│  │  • Public internet hosts                                │   │
│  │  • Live testnet (maya-testnet-1, ports 31100/31101)    │   │
│  │  • Production keys (any path with "prod" or "mainnet")  │   │
│  │  • GitHub, cloud APIs, external services               │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

Every fault injection command **must** verify lab context before executing.