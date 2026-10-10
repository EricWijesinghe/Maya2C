# Host Capacity Assessment

**Date:** 2026-10-09
**Host:** NITROZEUS (ROG Strix G18 G815LP)

## Hardware

| Resource | Value | Notes |
|---|---|---|
| **CPU** | Intel Core Ultra 9 275HX (24 cores, 24 threads) | No HT/SMT; max 2.7 GHz base |
| **Virtualization** | Firmware: Disabled; HypervisorPresent: True | WSL2/VirtualBox available; Hyper-V not enabled |
| **Physical RAM** | 32 GB (33.7 GB reported) | 5.1 GB available at measurement time |
| **Virtual Memory** | 48.5 GB max; 10.8 GB available | Page file on C: |
| **Storage D:** | 401 GB total; 259 GB free (36% used) | Primary workspace; cargo artifacts here |
| **Storage C:** | 341 GB total; 69 GB free (80% used) | OS; page file; avoid heavy writes |
| **Storage E:** | 213 GB total; 117 GB free (45% used) | WSL vhdx; secondary workspace |

## CPU Architecture

- 24 P-cores (no E-cores, no SMT)
- Virtualization extensions present but firmware-disabled
- WSL2 uses Hyper-V hypervisor (HypervisorPresent: True)
- No nested virtualization without Hyper-V enabled

## Software Environment

| Tool | Version | Location | Notes |
|---|---|---|---|
| **Rust** | 1.99.0-nightly (2026-07-14) | `~/.cargo/bin` | Pinned to nightly-2026-07-15 via rust-toolchain.toml |
| **Cargo** | 1.99.0-nightly | `~/.cargo/bin` | |
| **WSL2** | Ubuntu-24.04, Kali-Linux | Windows Feature | Both stopped; Ubuntu for eBPF/XDP builds |
| **Git** | Latest | Git for Windows | Bash available |
| **Docker** | Not installed | — | Podman not installed either |
| **Hyper-V** | Not enabled | — | Would conflict with WSL2 networking |
| **VirtualBox/VMware** | Not detected | — | Could be installed for full VM isolation |

## Network

- **Interfaces:** Physical Wi-Fi/Ethernet (single host IP)
- **Loopback:** 127.0.0.1/::1 (primary for validator lab)
- **WSL2:** Separate virtual NIC, NAT'd behind host
- **Available ports:** 1024–65535 (avoid 31100/31101 live testnet, 312xx peers, 32300+ attacknet)
- **Bandwidth tools:** `iperf3` (WSL), `ntttcp` (Windows), `pktmon` (Windows)

## Observability Tools

| Tool | Available | Notes |
|---|---|---|
| **cargo xtask attacknet** | Yes | 7 validators, 6 attacks, 3 rounds default |
| **cargo xtask localnet** | Yes | 4 validators over libp2p |
| **scripts/local_cluster.sh** | Yes | 12-node genesis ceremony dry-run |
| **tracing/tokio-console** | In crate deps | Requires `--features tokio-console` |
| **prometheus/grafana** | Not deployed | Can run in containers |
| **Wireshark/pktmon** | Windows | pktmon built-in |
| **bpftrace/ebpf** | WSL Ubuntu | Requires root |
| **perf** | WSL Ubuntu | Requires root |

## Resource Constraints for Validator Lab

### Memory Budget

| Component | Per Instance | 4 Validators | Notes |
|---|---|---|---|
| **RocksDB block_cache** | 32 MiB (configurable) | 128 MiB | Small-cache config from local_cluster.sh |
| **Rust async runtime** | ~50–100 MiB | 200–400 MiB | tokio + libp2p overhead |
| **DAG-BFT engine** | ~50 MiB | 200 MiB | In-memory certificates (GC_DEPTH=50) |
| **Process overhead** | ~20 MiB | 80 MiB | |
| **Total per validator** | **~150–200 MiB** | **~600–800 MiB** | With small-cache config |

**Available for validators:** ~4 GB (leaving 1 GB for host + monitoring + load generators)

### CPU Budget

| Component | Allocation |
|---|---|
| **Host OS + Claude + IDE** | 4–6 cores |
| **4 Validators** | 4–6 cores (1–1.5 each) |
| **Load generator** | 2–3 cores |
| **Monitoring/collection** | 1–2 cores |
| **WSL2/background** | 2–3 cores |
| **Reserve** | 4–6 cores |
| **Total** | **24 cores** |

### Storage Budget

| Component | Estimate |
|---|---|
| **Genesis + configs** | < 100 MB |
| **4 × chain databases** | 2–5 GB each (10–20 GB total) |
| **Logs (rotated)** | 1–2 GB |
| **Metrics/snapshots** | 1–2 GB |
| **Attack artifacts** | 500 MB |
| **Total** | **< 25 GB** (well within 259 GB free on D:) |

### Commit Charge (Virtual Memory) Constraint

**Critical:** Windows commit charge is the binding constraint, not physical RAM.

- Each `cargo build` process maps the node rlib (~2–3 GB commit each)
- 7 validators + build = ~20 GB commit during startup
- Current: 37.7 GB of 48.5 GB in use (78%)
- **Limit concurrent validators to 4** (not 7) for headroom
- Attacknet's 7 validators already caused OOM (mitigated by pausing peers)

## Capacity Verdict

| Scenario | Feasible? | Configuration |
|---|---|---|
| **4 validators + load gen + monitoring** | ✅ Yes | Primary target |
| **7 validators (current attacknet)** | ⚠️ Marginal | Requires pausing live peers; commit pressure |
| **4 validators in WSL2 VMs** | ⚠️ Marginal | Double memory overhead; nested virt not working |
| **Full VMs (VirtualBox)** | ❌ No | 32 GB RAM insufficient for 4× guest OS + host |

**Recommendation:** Use **process isolation on Windows host** (current attacknet model) with:
- 4 validators (not 7)
- Small RocksDB cache (32 MiB each)
- Separate data directories on D:\Maya2C-attacknet-v{1..4}
- Network isolation via distinct loopback ports
- Resource limits via Windows Job Objects or `cgroups` in WSL2