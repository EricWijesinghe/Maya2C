# Tool Inventory

**Date:** 2026-10-09
**Lab ID:** `maya2c-attacknet-lab-v1`

## Provisioning & Orchestration

| Tool | Version | Source | Purpose | Lab Use |
|---|---|---|---|---|
| **PowerShell 5.1 / 7.x** | 5.1.29680 | Windows | Primary orchestration | Validator lifecycle, Job Objects, network config |
| **WSL2 (Ubuntu-24.04)** | 2.4.x | Microsoft Store | Linux toolchain | eBPF/XDP, bpftrace, iperf3, network namespace tests |
| **Git for Windows** | 2.47+ | git-scm.com | Version control | Worktree management, report publishing |
| **cargo xtask** | Custom | `xtask/src/` | Build + test orchestration | `attacknet`, `localnet`, `sweep`, `status` |

## Virtualization & Isolation

| Tool | Available | Lab Use | Notes |
|---|---|---|---|
| **Windows Job Objects** | Native (P/Invoke) | CPU affinity, memory limits, process groups | Requires Rust `windows-rs` or PowerShell wrapper |
| **Windows Containers** | Not enabled | — | Requires Hyper-V; conflicts with WSL2 |
| **WSL2 Namespaces** | Yes (root) | Network isolation, cgroups v2 | `unshare`, `nsenter`, `systemd-run` |
| **VirtualBox** | Not installed | Full VM fallback | 32 GB RAM insufficient for 4× guests |
| **Hyper-V** | Disabled | — | Conflicts with WSL2 networking |
| **Firejail/Bubblewrap** | WSL only | Process sandboxing in Linux | Not on Windows host |

## Network Control & Simulation

| Tool | Available | Lab Use | Notes |
|---|---|---|---|
| **tc (traffic control)** | WSL2 (root) | Latency, loss, jitter, bandwidth, reorder | `tc qdisc add dev eth0 root netem ...` |
| **pktmon** | Windows native | Packet capture, filtering | `pktmon start --capture --pkt-size 128` |
| **Wireshark/tshark** | Windows/WSL | Deep packet analysis | GUI on Windows; CLI in WSL |
| **nftables/iptables** | WSL2 (root) | Firewall rules, port forwarding | Partition simulation |
| **socat/netcat** | Both | Connection testing, proxy | `socat TCP-LISTEN:33300 TCP:127.0.0.1:33301` |
| **toxiproxy** | Not installed | TCP proxy with fault injection | Can build from source; Go binary |
| **chaos-mesh** | No | K8s chaos engineering | Not applicable (no K8s) |
| **Custom Rust proxy** | Can build | Protocol-aware fault injection | libp2p frame parsing + corruption |

## Load & Traffic Generation

| Tool | Available | Lab Use | Notes |
|---|---|---|---|
| **cargo xtask attacknet** | Built-in | 6 built-in attacks | Baseline adversarial suite |
| **wrk / wrk2** | WSL/Windows | HTTP/RPC load | `wrk -t4 -c100 -d30s http://...` |
| **vegeta** | Not installed | HTTP load with rate shaping | Go binary; can cross-compile |
| **Custom Rust generator** | Can build | Protocol-native tx submission | Uses `maya2c-sdk` or raw JSON-RPC |
| **iperf3** | WSL2 | Bandwidth measurement | `iperf3 -s` / `iperf3 -c` |

## Fuzzing & Security Testing

| Tool | Available | Lab Use | Notes |
|---|---|---|---|
| **cargo fuzz** | In workspace | Structure-aware fuzzing | `crates/*/fuzz/` targets; libfuzzer |
| **libfuzzer** | Via cargo-fuzz | Coverage-guided | Requires `nightly` + `sanitizer` |
| **afl++** | WSL2 | Alternative fuzzer | More complex setup |
| **Kani** | WSL2 only | Formal verification | `cargo kani`; only works in WSL |
| **cargo audit** | `cargo install cargo-audit` | CVE scanning | Runs in CI; local on demand |
| **cargo deny** | Workspace | License/advisory/ban check | `cargo deny check` in sweep |
| **dudect** | `scripts/dudect.sh` | Constant-time testing | HQC decaps leakage found |
| **proptest** | In deps | Property-based testing | Fee market, consensus, VM |
| **custom exploit replays** | `crates/offsec-sandbox` | Known vulnerability replay | Isolated; opt-in feature |

## Monitoring & Observability

| Tool | Available | Lab Use | Notes |
|---|---|---|---|
| **Prometheus** | Not deployed | Metrics collection | Run in container or WSL |
| **Grafana** | Not deployed | Dashboards | Run in container or WSL |
| **Alertmanager** | Not deployed | Alerting | Run in container or WSL |
| **Vector / Fluent Bit** | Not deployed | Log aggregation | Can run as Windows service |
| **tokio-console** | In deps (feature) | Async runtime diagnostics | `--features tokio-console` + `console-subscriber` |
| **tracing / tracing-subscriber** | In deps | Structured logging | JSON output for ingestion |
| **Windows Event Log** | Native | System events | `Get-WinEvent` |
| **ETW / PerfView** | Native | Deep performance tracing | Heavy; use sparingly |
| **bpftrace / bcc** | WSL2 (root) | Kernel-level tracing | eBPF programs for network/storage |

## Debugging & Profiling

| Tool | Available | Lab Use | Notes |
|---|---|---|---|
| **gdb / lldb** | WSL2 / Windows | Debugger | `rust-gdb` via `rustup component add llvm-tools-preview` |
| **rr** | WSL2 only | Record/replay debugging | Deterministic replay; requires root |
| **perf** | WSL2 (root) | CPU profiling | `perf record -g -- cargo run ...` |
| **valgrind** | WSL2 | Memory errors | Slow; not for production runs |
| **heaptrack** | WSL2 | Memory allocation profiling | KDE tool; works on Rust |
| **cargo flamegraph** | `cargo install flamegraph` | Flame graphs | Requires `perf` on Linux |
| **Visual Studio Debugger** | Windows | Native debugging | PDB support; good for FFI |

## Test Infrastructure (Existing)

| Component | Location | Status | Notes |
|---|---|---|---|
| **attacknet** | `xtask/src/attacknet.rs` | ✅ Working | 7 validators, 6 attacks, 3 rounds |
| **localnet** | `xtask/src/devnet.rs` | ✅ Working | 4 validators over libp2p |
| **local_cluster.sh** | `scripts/local_cluster.sh` | ✅ Working | 12-node genesis ceremony |
| **bft_node_tests** | `crates/node/tests/bft_node_tests.rs` | ✅ 376 tests | Unit + integration |
| **bft_staking_tests** | `crates/node/tests/bft_staking_tests.rs` | ✅ 6 tests | Stake-weighted committees |
| **chaos_simulator** | `crates/node/tests/chaos_simulator.rs` | ✅ Ported to maya-sim | Deterministic simulation |
| **latency_sim_tests** | `crates/node/tests/latency_sim_tests.rs` | ✅ Real libp2p | In-memory transport |
| **fuzz targets** | `crates/*/fuzz/` | 🔍 Need audit | Run `cargo fuzz list` |

## Missing / To Acquire

| Tool | Priority | Acquisition Plan |
|---|---|---|
| **toxiproxy** | High | Cross-compile Go binary for Windows; or run in WSL2 |
| **Prometheus/Grafana stack** | High | Docker Compose in WSL2; or Windows binaries |
| **Vector (log shipper)** | Medium | Windows binary from GitHub releases |
| **vegeta** | Medium | Go cross-compile for Windows |
| **cargo-fuzz targets audit** | High | Run `cargo fuzz list` in each crate; document |
| **Kani proof inventory** | Medium | `cargo kani --list` in WSL2 |
| **Network namespace manager** | Medium | Rust crate `netns` or shell wrapper for WSL2 |

## Selection Criteria Applied

1. **Host compatibility** → Windows-native preferred; WSL2 for Linux-only
2. **Repository compatibility** → Existing `cargo xtask` and test infrastructure first
3. **Isolation quality** → Process > container > namespace > none
4. **Reproducibility** → Deterministic seeds, version-pinned tools
5. **Resource efficiency** → No full VMs; shared kernel via WSL2 when needed
6. **Security** → No privileged daemons on host; root only in WSL2
7. **Maintainability** → Fewer tools, well-documented
8. **Zero-cost** → All selected tools are free/open-source
9. **Licensing** → MIT/Apache-2.0/BSD preferred; GPL avoided for distribution

## Tool Versions to Pin

```toml
# In a future `tools.toml` or `dev-dependencies`
toxiproxy = "2.7.0"
prometheus = "2.54.0"
grafana = "11.1.0"
vector = "0.42.0"
vegeta = "12.12.0"
iperf3 = "3.17"
bpftrace = "0.21.0"
```

All tools to be installed in `D:\Maya2C-tools\` with versioned subdirectories, added to `PATH` only during lab sessions.