# Master System Trajectory

The order in which the build sessions go, as prompts 1–160, grouped into
domains. This file is the *plan*. It is not a statement of what the tree
contains. For that, read the status table in [CLAUDE.md](../CLAUDE.md) and
[architecture-vision.md](architecture-vision.md), which tag every component
SHIPPED, RESEARCH or PLANNED. If the two disagree, the status tags win until
someone reconciles them in writing.

Recorded 2026-09-13.

## Progress

- **Completed:** through Prompt ~47 of 160.
- **Next:** Prompt 48.
- **Active domain:** Consensus & State Engine (Prompts 26–50).

## Domains

| # | Prompts | Domain | Deliverables |
|---|---|---|---|
| 1 | 1–25 | Post-Quantum Cryptography | NIST Category 5 algorithms (ML-KEM-1024, ML-DSA-87, SLH-DSA), ETSI QKD interfaces, lattice HTLC atomic swaps |
| 2 | 26–50 | **Consensus & State Engine (current, ~47)** | Multi-shard Narwhal/Tusk asynchronous DAG, recursive Halo2 ZK-STARK state folding, Cranelift WASM VM |
| 3 | 51–75 | Physical & Hardware Mesh | eBPF/XDP zero-copy networking, deep-sea acoustic and neutrino transport, CXL 3.1 memory disaggregation, photonic acceleration |
| 4 | 76–100 | Autonomous Governance & AI | Spiking Neural Network edge defense, Z3 formal logic verifiers, ISO 20022 messaging, nuclear/fusion smart-grid balancing |
| 5 | 101–125 | Formal Verification & Tools | Lean 4 safety proofs, wgpu multi-backend miners, Leptos block explorer, threshold MPC custody frameworks |
| 6 | 126–130 | Production Pipeline | Deterministic Docker builds, Terraform/Helm IaC, Prometheus/Grafana SRE dashboards, zero-trust RPC gateways |
| 7 | 131–135 | Security & Audit Hardening | Binius/Halo2 post-quantum ZK upgrades, EIP-1559 fee market enforcement, DAG consensus binding, profile optimization |
| 8 | 136–140 | Type-VII UI & Launch Harness | Three.js WebGL 3D holographic visualizer, Tauri 2.0 wallet GUI, interactive credential setup scripts, cloud orchestrator |
| 9 | 141–145 | Day-2 Autonomy & Ecosystem | Continuous LibAFL fuzzing, WASM runtime hot-swapping, validator staking/slashing, polyglot SDKs, self-healing agentic SRE |
| 10 | 146–150 | Terminal Execution Suite | Automated cargo cleanup, deployment secret vault generators, automated Terraform triggers, live RPC smoke testing |
| 11 | 151–155 | Institutional & Orbital Scale | Quantum-safe AMM, RWA ZK-KYC compliance, developer grant engines, genesis admin key burning, satellite relays |
| 12 | 156–160 | Theoretical Horizon | Interstellar light-cone consensus, Dyson photonic compute, AI self-mutation engines, 1,000-year quartz archival, total autonomous handover |

## Where the plan and the tree disagree

These were open on 2026-09-13. Each one gets settled before the prompt that
would touch it writes any code, not afterwards.

1. **Security category.** Domain 1 names Category 5 (ML-KEM-1024, ML-DSA-87).
   The tree ships ML-KEM-768 and ML-DSA-65, and critical invariant 4 compiles
   `fips204` with only the `ml-dsa-65` set. Moving to 87 means changing that
   invariant on purpose, not adding a second set alongside it.
2. **Consensus deliverables that are dark.** Narwhal/Tusk (`blockgraph`) and
   halo2 (`zkml`) are RESEARCH: nothing in consensus calls either of them.
   Halo2 zkML verifies inference proofs. It is not STARK state folding, and it
   is not post-quantum.
3. **Deliverables already in the tree.** ISO 20022 (`iso20022`), wgpu miner,
   Leptos explorer, MPC custody (`custody-mpc`), EIP-1559 (`fee-market`, dark),
   Tauri wallet, and lattice HTLCs (`htlc-lattice`, dark). A later prompt for
   one of these extends what exists. It does not start over.
4. **Collisions with invariants.**
   - WASM runtime hot-swapping (domain 9) against invariant 13: no governed
     value is a program.
   - AI self-mutation (domain 12) against invariants 12 and 13.
   - Satellite relays (domain 11) and light-cone consensus (domain 12) against
     invariant 9, the same collision already listed for relativistic clock
     sync.
5. **Neural gas estimation, taken out of order.** Built 2026-09-14 while the
   trajectory stood at Prompt 48. Its brief collided with invariant 20 (float
   inference in consensus), invariant 13 (weights as a governable program) and
   invariant 28 (node-local features such as memory allocation depth), and asked
   for zkML where re-execution is ~10⁴× cheaper. Reconciled as a bounded,
   integer, compiled-in gain on the dark `fee-market` rule —
   [neural-gas.md](neural-gas.md). A later EIP-1559 enforcement prompt
   (domain 7) extends it; it does not start over.
