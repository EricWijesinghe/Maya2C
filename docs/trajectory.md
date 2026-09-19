# Master System Trajectory

The order in which the build sessions go, as prompts 1–160, grouped into
domains. This file is the *plan*. It is not a statement of what the tree
contains. For that, read the status table in [CLAUDE.md](../CLAUDE.md) and
[architecture-vision.md](architecture-vision.md), which tag every component
SHIPPED, RESEARCH or PLANNED. If the two disagree, the status tags win until
someone reconciles them in writing.

Recorded 2026-09-13.

## Progress

- **Completed:** through Prompt ~48 of 160. Prompt 48 was elastic shard
  auto-scaling over `blockgraph` (2026-09-14), reconciled as a per-node map —
  see [blockgraph.md](blockgraph.md#elastic-shards).
- **Next:** Prompt 49.
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
6. **Stateless execution with lattice vector commitments, taken out of order.**
   Built 2026-09-14 while the trajectory stood at Prompt 48. Its brief asked for
   lattice polynomial commitments with sub-kilobyte witnesses, and for
   validators that read no state. The first two collide with each other — no
   transparent lattice commitment opens in under a kilobyte, and the state root
   was already post-quantum — and the third collides with the end-of-block
   passes in `stage_block`, which read state no transaction names. Reconciled
   as a dark sparse-tree accounts root with BLAKE3 commitments, a measured
   Ring-SIS research backend, and stateless verification of transfer-only
   blocks — [stateless.md](stateless.md). Recursive state folding (domain 2)
   and witness gossip extend it; they do not start over.
7. **Byzantine self-healing network guard, taken out of order.** Built
   2026-09-15 while the trajectory stood at Prompt 49; it belongs to network
   security, not Consensus & State. Its brief assumed a validator set ("40%
   malicious nodes", "double proposals", "honest nodes construct consensus
   proofs"), which proof of work does not have. Reconciled as gossip
   validation, gossipsub P4 scoring, an expiring quarantine, connection caps
   and a bounded parent fetch — [peer-health.md](peer-health.md). An eclipse
   defence beyond connection caps, and fork-aware sync, extend it.
8. **Confidential federated training, taken out of order.** Built 2026-09-15
   while the trajectory stood at Prompt 49. Its brief put SGX / SEV-SNP enclaves
   at the centre, and the status table had already flagged that TEE attestation
   collides with invariant 11: a vendor's ECDSA signature is a trusted party,
   and not a post-quantum one. It also asked differential privacy to "prevent"
   reconstruction. Reconciled as off-chain secure aggregation over ML-KEM plus
   integer discrete-Gaussian DP with a reported ε, and an attestation hook that
   verifies nothing yet — [confidential-ai.md](confidential-ai.md).
9. **eBPF/XDP packet accelerator, taken out of order.** Built 2026-09-15 while
   the trajectory stood at Prompt 49; it belongs to Physical & Hardware Mesh
   (domain 3). Its brief had three problems.
   - It asked XDP to inspect P2P frames, but every P2P byte is Noise + ML-KEM
     ciphertext.
   - It asked for flatbuffers parsed under 100 µs at 100 Gbps, but the eBPF
     verifier cannot walk flatbuffers' offset tables, and parsing is nanoseconds
     next to the AEAD.
   - It asked for a benchmark "proving" 5×, but a benchmark cannot be written to
     prove a number fixed in advance.

   Reconciled as follows:
   - a separate UDP block relay;
   - a fixed 56-byte header the kernel judges structurally (a blocklist fed by
     authenticated quarantines, a token bucket, the exact length);
   - AF_XDP delivery;
   - XChaCha20-Poly1305 chunks under keys agreed over the post-quantum connection;
   - a benchmark that reports kernel UDP against AF_XDP and `XDP_DROP` without
     asserting a ratio.

   See [ebpf-net.md](ebpf-net.md). Off by default, and not measured on a
   native-XDP NIC.
10. **Autonomous red-team fuzzing module, taken out of order.** Built
    2026-09-15 while the trajectory stood at Prompt 49; it belongs to Day-2
    Autonomy (domain 9, LibAFL). Its brief had four collisions.
    - It put LibAFL and Triton/Z3 "directly into the development and testing
      execution path", which is exactly what `fuzz/`'s separate workspace keeps
      out of the node graph and away from Kani (invariants 1, 6). Triton is a
      C++ binary DSE framework with no surface on a Rust WASM chain.
    - It asked for "memory corruption" and "state race conditions": safe Rust
      has neither outside the `unsafe` islands and a single-threaded apply path.
      The real properties are panic/DoS resistance and deterministic
      re-execution (invariant 24, directive 2).
    - It asked the pipeline to "generate patch diffs" — machine-authored
      consensus changes, which invariant 13 forbids in spirit.
    - It asked a `tests/` harness to "verify 100% crash resistance" over a
      million payloads, which the stable-toolchain workspace cannot link LibAFL
      to do, and which fuzzing cannot prove regardless.

    Reconciled as `offsec-sandbox` (a separate workspace, LibAFL engine, Z3
    off by default over the arithmetic crates only), structure-aware mutators,
    a triage-and-regression-stub pipeline (no auto-patch), and an in-tree
    `tests/fuzz_harness.rs` that replays seeded mutations and is worded "no
    crash across N inputs", never "100%". See [offsec-sandbox.md](offsec-sandbox.md).
11. **Zero-trust threat-intel registry, taken out of order.** Built 2026-09-15
    while the trajectory stood at Prompt 49; it is the PLANNED "ZK-SIEM threat
    mesh", nearest to Autonomous Governance & AI (domain 4, edge defense). Its
    brief had six collisions.
    - It asked for zero-knowledge proofs of DDoS floods and port scans. Neither
      leaves anything a third party can verify — a UDP source is forgeable — and
      the one offence that does (bytes that fail a check) needs no ZK.
    - It asked for a consensus-driven score, but proof of work has no validator
      set and identities are free, so a counted-by-head score is Sybil-owned.
    - It asked for IP quarantine recorded in state. No proof binds an IP to a
      key, so every recorded address is a censorship lever.
    - It asked for "2 DAG rounds", and nothing in consensus has rounds.
    - It asked one worker to push firewall rules "across enterprise
      infrastructure", which is a lateral-movement control plane.
    - It implied the registry stops a DDoS; free keys mean it cannot.

    Reconciled as verified evidence from ed25519 gossip signatures (invalid
    signature, `tx_root` mismatch), one conviction confirms, indicators keyed by
    author only, a per-host worker that maps authors to its own connections, and
    a test pinning enforcement within 2 **blocks** — plus one pinning that a
    valid-byte flood produces no indicator. See [threat-intel.md](threat-intel.md).
12. **DePIN hardware attestation (IoT anchor), taken out of order.** Built
    2026-09-16 while the trajectory stood at Prompt 49; it belongs to Physical
    & Hardware Mesh (domain 3). Its brief had seven collisions.
    - "Post-quantum lightweight signatures" do not exist, and invariant 4
      compiles only ML-DSA-65.
    - Software is not a PUF.
    - TPM 2.0 and secure elements generate no ML-DSA keys, and their vendor
      attestation is a classical trusted party (invariant 11).
    - Enrollment cannot prove genuine silicon.
    - Consensus has no DAG, and a signature per reading is 3.3 KB.
    - A signature cannot validate physics.
    - A chain senses no tampering.

    Reconciled as ML-DSA-65 device keys from a PUF fuzzy extractor or a
    PCR-sealed TPM seed, owner enrollment with proof of possession,
    Merkle-rooted batches with bounds flagged rather than refused, and signed
    tamper events plus conflicting-batch evidence as the only on-chain
    tamper signals. See [iot-anchor.md](iot-anchor.md).
