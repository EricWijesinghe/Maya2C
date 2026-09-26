# 07 — Networking, transports and time

**Machine:** cloud VM, 4 × Intel Xeon @ 2.80 GHz, 15 GiB RAM, Ubuntu 24.04.4,
kernel 6.18.44, `nightly-2026-07-15`.

## Verdict

| DONE WHEN criterion | State | Evidence |
|---|---|---|
| P2P and PQ-handshake tests pass | **Met** | §1 |
| eBPF program loads in a CI VM (or skips with a reason) | **Skips with a documented reason** here: the XDP test needs `CAP_NET_ADMIN` and the `xdp` feature; the CI job `ebpf-net.yml` is where it loads | §2 |
| Transport sims pass | **Met** (new) | §3 |
| `reports/07-network.md` complete | this file | — |

## 1. P2P and post-quantum handshakes (full workspace run)

```
tests/network_tests.rs           :: test result: ok. 6 passed; 0 failed; ... in 2.89s
tests/pq_transport_tests.rs      :: test result: ok. 6 passed; 0 failed; ... in 1.11s
tests/byzantine_guard_tests.rs   :: test result: ok. 3 passed; 0 failed; ... in 25.01s
tests/radio_transport_tests.rs   :: test result: ok. 14 passed; 0 failed; ... in 0.04s
```

Plus `dualkem_latency_tests.rs` and `dualkem_negotiation_tests.rs` (both in
the passing run): DualKem (ML-KEM + HQC) negotiation and per-session rotation.
The dual-KEM transport is `off` by default (`docs/pq-transport.md` lists the
three conditions for changing that; HQC decapsulation is not constant-time,
ADR-009).

## 2. eBPF / XDP

`hal/ebpf-net` builds by default without `aya` (kernel UDP socket path); the
XDP loader and AF_XDP rings are behind the `xdp` feature, and
`hal/ebpf-net/tests/xdp_veth.rs` needs root and a veth pair
(`scripts/xdp_netns.sh`). Not run on this VM. `benches/ebpf_bench.rs` (5×
goal) is not written.

## 3. Transport HAL models (new: `hal/link-sim`) — all SIM or RESEARCH

`cargo test -p maya-link-sim` → **11 passed** (27.5 s debug):

| Test | Rule it pins |
|---|---|
| `an_fso_fade_beyond_15_db_falls_back_to_rf` | FSO above a 15 dB fade reports down; `choose` falls back to Ku-band RF |
| `subsea_acoustic_latency_follows_the_speed_of_sound` | 15 km at 1,500 m/s = 10 s |
| `oam_phase_front_recovery_keeps_more_modes` | adaptive optics keeps more OAM channels under turbulence |
| `a_5000_km_repeater_chain_needs_purification_to_pass_the_fidelity_gate` | Werner-state swapping over 50 × 100 km fails F > 0.95 raw, passes with two BBPSSW rounds |
| `qkd_is_mixed_in_below_11_percent_qber_and_dropped_above` | QKD key only ever mixed into the ML-KEM secret; QBER ≥ 11% drops it |
| **`entanglement_keys_require_classical_channel`** | entangled pairs give ~75% raw agreement (no key); sifting over the classical transcript gives identical keys |
| `a_signed_pq_transaction_never_fits_a_radio_frame` | a 3,309-byte signature cannot fit 255 bytes: the frame carries a hash |
| `a_neutrino_link_is_research_and_repetition_decoding_recovers_bits` | RESEARCH class; majority decoding at 40% bit error |
| `orbital_delays_line_of_sight_and_doppler` | Earth–Mars 3.0–22.3 light-min, Earth–Moon 1.28 s, LEO line of sight, Doppler sign |
| `mars_sub_dag_roots_reach_earth_in_order_and_do_not_slow_earth` | two DAG-BFT clusters; Mars anchors batched to Earth over 3 and 22 min: every Earth node sees the same gap-free root sequence |
| `store_and_forward_recovers_full_state_through_30_percent_loss` | a 3-hop relay with 30% loss per hop delivers all 500 chunks |

## 4. Time (new: `crates/timing`)

`cargo test -p maya-timing` → **5 passed**:

- Marzullo fusion: a spoofed source claiming a 3 ms offset with 10 ns
  confidence is **outvoted**, not averaged; no majority → "time unknown".
- Relativity: GPS orbit reproduces SR −7.2 µs/day, GR +45.7 µs/day, net
  +38.5 µs/day; the ISS clock runs slow.
- `within_drift` — the integer, symmetric bounded-drift check — is the only
  time rule a consensus path may use.

**Precision actually achieved:** nothing was measured against a reference
clock on this VM. Physically available bounds are documented in
`crates/timing/src/lib.rs` (NTP 1–50 ms, PTP 10 ns–1 µs, GNSS 20–50 ns);
sub-picosecond network agreement is stated as unachievable.

## 5. Not built

ETSI GS QKD 014 client; DTN Bundle Protocol v7; libp2p gossipsub topic rename
to `/maya2c/dag/1` (no DAG gossip until ADR-015 wiring); DPDK/terabit backplane
design doc; ML anomaly detector feeding XDP maps; DDoS lab flood test; the
40%-malicious fail-safe proof beyond the 2/2 no-quorum halt in `dag-bft`.
