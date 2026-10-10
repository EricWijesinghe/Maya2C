# Network Topology

**Lab ID:** `maya2c-attacknet-lab-v1`
**Date:** 2026-10-09

## Physical Network

```
Host: NITROZEUS (Windows 11)
├── Physical NIC: Wi-Fi / Ethernet (single upstream)
├── Loopback: 127.0.0.1/8, ::1/128
├── WSL2 Virtual NIC: 172.16.x.x/20 (NAT'd)
└── Hyper-V Switch: Not enabled (would conflict with WSL2)
```

## Lab Logical Network (Loopback Only)

All lab traffic stays on `127.0.0.1` — no external interfaces used.

### Port Allocation

| Range | Purpose | Instances |
|---|---|---|
| **33300–33303** | Validator P2P (libp2p) | 4 |
| **33310** | Bootnode P2P | 1 |
| **33320** | Adversarial Peer P2P | 1 |
| **33330–33339** | Reserved: network fault injection proxies | 10 |
| **34300–34303** | Validator JSON-RPC | 4 |
| **34310** | RPC Gateway (REST) | 1 |
| **34311** | RPC Gateway (WebSocket) | 1 |
| **34320–34329** | Reserved: RPC fault injection proxies | 10 |
| **35300–35303** | Validator Metrics (Prometheus) | 4 |
| **35310** | Prometheus Server | 1 |
| **35311** | Grafana | 1 |
| **35312** | Alertmanager | 1 |
| **35313** | Vector (log ingest) | 1 |
| **35314** | Test Controller RPC | 1 |

### Validator Network Config

Each validator `v{N}` (`N=1..4`):

```toml
# D:\Maya2C-attacknet-v{N}\config.toml
[network]
p2p_port = 33299 + N        # 33300, 33301, 33302, 33303
bootnodes = ["/ip4/127.0.0.1/tcp/33310"]  # Single bootnode
dual_kem = "off"            # ML-KEM-768 handshake (production)

[rpc]
listen = "127.0.0.1:34299 + N"  # 34300–34303
rate_limit_per_second = 1000
rate_limit_burst = 2000

[metrics]
listen = "127.0.0.1:35299 + N"  # 35300–35303

[storage]
block_cache_mib = 32
write_buffer_mib = 8
max_open_files = 256
```

### Bootnode Config

```toml
# D:\Maya2C-attacknet-bootnode\config.toml
[network]
p2p_port = 33310
bootnodes = []  # Is the bootnode
dual_kem = "off"

[rpc]
listen = "127.0.0.1:34310"  # Gateway reaches here for peer info
```

### Adversarial Peer Config

```toml
# D:\Maya2C-attacknet-adversary\config.toml
[network]
p2p_port = 33320
bootnodes = ["/ip4/127.0.0.1/tcp/33310"]
dual_kem = "off"
# No [validator] section — no consensus key

[rpc]
listen = "127.0.0.1:34320"  # Controller commands it here
```

## Network Segments for Fault Injection

Each segment is a **logical grouping** of connections that can be independently impaired.

### Segment Definitions

| Segment | Endpoints | Injection Method |
|---|---|---|
| **S1: Validator↔Validator** | 33300↔33301, 33300↔33302, 33300↔33303, 33301↔33302, 33301↔33303, 33302↔33303 | `tc netem` on WSL2 veth pairs |
| **S2: Validator→Bootnode** | 33300–33303 → 33310 | `tc netem` on bootnode ingress |
| **S3: Bootnode→Validator** | 33310 → 33300–33303 | `tc netem` on validator ingress |
| **S4: RPC Client→Validator** | Any → 34300–34303 | `toxiproxy` on each RPC port |
| **S5: RPC Gateway→Validator** | 34310 → 34300–34303 | `toxiproxy` on gateway egress |
| **S6: Adversary→Validator** | 33320 → 33300–33303 | Direct (adversary controls its send) |
| **S7: Validator→Adversary** | 33300–33303 → 33320 | `tc netem` on adversary ingress |
| **S8: Metrics Scraping** | 35310 → 35300–35303 | `toxiproxy` on Prometheus egress |
| **S9: Inter-Validator Gossip** | All libp2p substreams | Protocol-aware proxy (Rust) |

### WSL2 Network Namespace Layout (for `tc`)

```
WSL2 (Ubuntu-24.04)
├── init namespace (eth0: 172.16.x.x)
│
├── ns-validator-1 (veth1a ↔ veth1b)
│   └── veth1a: 10.200.1.2/30 → maps to host 127.0.0.1:33300 via socat
│
├── ns-validator-2 (veth2a ↔ veth2b)
│   └── veth2a: 10.200.2.2/30 → maps to host 127.0.0.1:33301 via socat
│
├── ns-validator-3 (veth3a ↔ veth3b)
│   └── veth3a: 10.200.3.2/30 → maps to host 127.0.0.1:33302 via socat
│
├── ns-validator-4 (veth4a ↔ veth4b)
│   └── veth4a: 10.200.4.2/30 → maps to host 127.0.0.1:33303 via socat
│
├── ns-bootnode (veth-ba ↔ veth-bb)
│   └── veth-ba: 10.200.10.2/30 → maps to host 127.0.0.1:33310 via socat
│
├── ns-adversary (veth-ad ↔ veth-ad)
│   └── veth-ad: 10.200.20.2/30 → maps to host 127.0.0.1:33320 via socat
│
└── ns-gateway (veth-gw ↔ veth-gw)
    └── veth-gw: 10.200.30.2/30 → maps to host 127.0.0.1:34310 via socat
```

**Note:** This adds complexity. **Simpler alternative:** Run validators on Windows host loopback, use `pktmon` for capture, and `toxiproxy` (Windows binary) for L7 fault injection. Reserve WSL2 namespaces for **eBPF/XDP wire-level tests only**.

### Recommended: Hybrid Approach

| Fault Type | Tool | Location |
|---|---|---|
| **Process kill/restart** | PowerShell Job Objects | Windows host |
| **TCP latency/loss/jitter/reorder** | `toxiproxy` (Windows) | Host loopback ports |
| **Packet corruption/drop (L4)** | `pktmon` filter + drop | Windows host |
| **Protocol-aware corruption (L7)** | Custom Rust proxy | Host process |
| **eBPF/XDP wire tests** | `tc` + `xdp` | WSL2 Ubuntu (root) |
| **Bandwidth limit** | `toxiproxy` rate limit | Host loopback |

## Partition Simulation Scenarios

| Scenario | Segments Affected | `tc` / `toxiproxy` Config |
|---|---|---|
| **Minority partition (1 validator)** | S1: isolate v1 from v2,v3,v4 | `tc qdisc add dev veth1b root netem loss 100%` |
| **Majority partition (3 validators)** | S1: isolate v4 from v1,v2,v3 | `tc qdisc add dev veth4b root netem loss 100%` |
| **Symmetric partition (2+2)** | S1: v1,v2 ↔ v3,v4 cut | Two `tc` rules on cross edges |
| **Asymmetric (v1→others ok, others→v1 drop)** | S1: directional | `tc filter` with `u32 match` on src/dst port |
| **Bootnode loss** | S2, S3: drop all to/from 33310 | `tc` on bootnode veth |
| **Gateway isolation** | S5: drop gateway→validators | `toxiproxy` toxic on gateway egress |
| **RPC overload** | S4: latency + rate limit | `toxiproxy` latency + bandwidth toxics |
| **Adversary flood** | S6: adversary sends max rate | Adversary process controls |

## Latency/Jitter/Duplication/Reorder Profiles

| Profile | `tc netem` Command | Use Case |
|---|---|---|
| **Baseline (LAN)** | `delay 1ms 0.1ms` | Normal operation |
| **Cross-region** | `delay 50ms 10ms` | Geo-distributed simulation |
| **Satellite** | `delay 600ms 50ms` | Space comms (PLANNED) |
| **Jitter burst** | `delay 10ms 50ms distribution normal` | Congestion |
| **Packet loss 1%** | `loss 1%` | Mild degradation |
| **Packet loss 10%** | `loss 10%` | Severe degradation |
| **Duplication 5%** | `duplicate 5%` | Network glitch |
| **Reorder 25%** | `reorder 25% gap 5` | Path asymmetry |
| **Corruption 0.1%** | `corrupt 0.1%` | Bit errors |

## Bandwidth Constraints

| Constraint | `toxiproxy` Config | `tc` Config |
|---|---|---|
| **100 Mbps** | `bandwidth 100000` (KB/s) | `rate 100mbit` |
| **10 Mbps** | `bandwidth 10000` | `rate 10mbit` |
| **1 Mbps** | `bandwidth 1000` | `rate 1mbit` |
| **100 Kbps** | `bandwidth 100` | `rate 100kbit` |

## DNS Failure Simulation

- Not applicable (all loopback IPs)
- For hostname resolution tests: add entries to `C:\Windows\System32\drivers\etc\hosts` then remove

## Connection Reset Simulation

| Method | Command |
|---|---|
| **RST on establish** | `toxiproxy` toxic: `reset` on connect |
| **RST mid-stream** | `toxiproxy` toxic: `reset` after N bytes |
| **FIN (clean close)** | `toxiproxy` toxic: `close` after N bytes |

## Verification Commands

```powershell
# Verify all lab ports listening
netstat -an | findstr "3330[0-3] 33310 33320 3430[0-3] 34310 34311 3530[0-3] 3531[0-4]"

# Verify no cross-talk with live testnet
netstat -an | findstr "31100 31101 312" && echo "CONFLICT!" || echo "Clean"

# Capture validator 1 P2P traffic
pktmon start --capture --pkt-size 1500 --filter "TCP.Port == 33300"
# ... run scenario ...
pktmon stop
pktmon format --file v1.pcapng
```

## Network Topology Diagram (Mermaid)

```mermaid
graph TB
    subgraph Host[Windows Host: NITROZEUS]
        subgraph Validators[Validator Processes]
            V1[Validator 1\nP2P:33300 RPC:34300 Metrics:35300]
            V2[Validator 2\nP2P:33301 RPC:34301 Metrics:35301]
            V3[Validator 3\nP2P:33302 RPC:34302 Metrics:35302]
            V4[Validator 4\nP2P:33303 RPC:34303 Metrics:35303]
        end
        
        subgraph Infra[Infrastructure]
            BN[Bootnode\nP2P:33310]
            GW[RPC Gateway\nREST:34310 WS:34311]
            ADV[Adversarial Peer\nP2P:33320 RPC:34320]
        end
        
        subgraph Monitor[Monitoring (WSL2 Containers)]
            PROM[Prometheus:35310]
            GRAF[Grafana:35311]
            ALERT[Alertmanager:35312]
            VEC[Vector:35313]
        end
        
        CTRL[Test Controller]
    end
    
    BN --- V1
    BN --- V2
    BN --- V3
    BN --- V4
    V1 --- V2
    V1 --- V3
    V1 --- V4
    V2 --- V3
    V2 --- V4
    V3 --- V4
    ADV -.-> V1
    ADV -.-> V2
    ADV -.-> V3
    ADV -.-> V4
    GW --> V1
    GW --> V2
    GW --> V3
    GW --> V4
    PROM --> V1
    PROM --> V2
    PROM --> V3
    PROM --> V4
    PROM --> GW
    PROM --> BN
    PROM --> ADV
    CTRL --> V1
    CTRL --> V2
    CTRL --> V3
    CTRL --> V4
    CTRL --> ADV
    CTRL --> GW
```