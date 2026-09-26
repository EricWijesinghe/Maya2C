<p align="center">
  <img src="../logo-assets/website/Header-Logo_250x100.png" width="250" height="100" alt="Maya2C"/>
</p>

<p align="center">
  The target architecture, and how much of it exists.
</p>

---

# Architecture Vision

Maya2C is scoped as an autonomous, post-quantum Layer-1 **monolithic
ecosystem** — one chain that owns its cryptography, its execution, its
transport, and the hardware it attests, rather than a settlement layer that
delegates each of those to something else. The stated target spans software,
hardware, space communications, bio-computing, and quantum physics: roughly 75
subsystems, of which 30 are workspace members today.

This document exists because the gap between that target and the tree is the
single most expensive thing to rediscover. A subsystem that was never written
and a subsystem that is written but deliberately dark look identical from
outside, and both look identical to one that shipped. Confusing them costs
either a search for code that does not exist, or — worse — a safety argument
that rests on a component nothing implements.

So every entry below carries a status, and the status is the load-bearing part:

| Tag | Meaning |
|---|---|
| **SHIPPED** | In the tree, reachable, with tests. Named crate or path. |
| **RESEARCH** | In the tree and tested, but *nothing in consensus calls it*. Usually behind an activation height of `u64::MAX`. Changing that is a decision somebody writes down. |
| **PLANNED** | No code yet. Design intent only. Do not go looking for it. |

`CLAUDE.md` carries the condensed version of this table and is the authority
for what a session may assume. An entry graduates PLANNED → RESEARCH → SHIPPED
by acquiring, in that order, a crate and a test, and then a caller in
consensus. It earns a numbered invariant in `CLAUDE.md` only at the point a
test pins the behaviour that must not change.

---

## 1. Cryptography and key management

The post-quantum floor is the part of the design that is finished. Every
signature on the chain is a hybrid pair and both halves must verify; see
[hybrid-signatures.md](hybrid-signatures.md).

| Component | Status | Where | Notes |
|---|---|---|---|
| ML-KEM-768 (FIPS 203) | **SHIPPED** | `crypto-pq` | Session establishment over Noise — [pq-transport.md](pq-transport.md) |
| ML-DSA-65 (FIPS 204) | **SHIPPED** | `crypto-pq`, `fips204` | Only the `ml-dsa-65` parameter set is compiled. Invariant 4 |
| SLH-DSA (FIPS 205) | **SHIPPED** | `crypto-pq` | The hash-based half of the pair. Must stay the instantiating crate — invariant 2 |
| HQC (Hamming Quasi-Cyclic) | **SHIPPED** | `crates/crypto-pq/src/hqc.rs` | Second KEM, available as a fallback. Conditions for enabling it are in [pq-transport.md](pq-transport.md) |
| Shielded pool (STARK joinsplit) | **SHIPPED** | `crates/zk-stark/src/pool/`, `crates/node/src/state/shielded.rs` | 2-in-2-out joinsplit proved as a Plonky3 STARK — no setup, hash-based soundness. Replaced the Groth16/BLS12-381 pool (`zk-privacy`, removed 2026-09-21). Proof ~383 KB; 113 bits conjectured / 99 proven. Mainnet blocked on `CIRCUIT_IS_AUDITED = false` |
| Zero-knowledge proofs (halo2/KZG) | **PLANNED** | — | Retired 2026-09-21 with `crates/zkml`: neither transparent nor post-quantum (ADR-008). To return as a Plonky3 STARK |
| m-of-n ML-DSA multisig accounts | **REAL** | `crates/crypto-pq/src/multisig.rs`, `crates/node/src/core/multisig_tx.rs` | ADR-011. The account is the policy's address, so the chain enforces the policy with no registry and no admin key; each member signs with a NIST-vectored suite. Wire v8, live from genesis with the v7 envelope (ADR-013); a 3-of-5 spend moves value through the apply path in `crates/node/tests/suite_envelope_live_tests.rs` |
| Threshold lattice signing | **RESEARCH** | `crates/custody-mpc/src/threshold.rs` | Feature `threshold-lattice`: the interface a dealerless, aggregating scheme must meet, and `activate()` refusing. No peer-reviewed scheme is chosen (ADR-011 lists candidates and criteria) |
| MPC-TSS threshold custody (*m*-of-*n*) | **SHIPPED** | `custody-mpc` | Dealerless Pedersen VSS, ML-KEM-sealed shares. Protects the 32-byte chain key, not either signature — invariants 18, 19 and [custody-mpc.md](custody-mpc.md) |
| Verifiable random function (RFC 9381) | **SHIPPED** | `vrf` | Suite octet `0x03`; the RFC's own vectors are pinned — invariant 10 |
| Threshold-encrypted mempool | **SHIPPED** | `mev`, `crates/node/src/sealed/`, `crates/node/src/state/sealed_exec.rs` | A miner orders transactions it cannot read. `settle_sealed` runs every block from `stage_block`; the crate itself stays chain-free, curve arithmetic and an AEAD. Optional at genesis like the oracle — a chain with no committee accepts no envelope. **Not post-quantum**: the KEM half is Ristretto ElGamal, because threshold ElGamal has no ML-KEM analogue |
| Self-sovereign identity records | **RESEARCH** | `identity`, `crates/node/src/state/identity.rs` | `did:maya2c:<address>` — the 32-byte address, which is blake3 over *both* public keys, so a DID inherits the hybrid binding `Transaction::sender()` argues for. A 1,984-byte hybrid key would be a 2,712-character DID |
| DID key rotation and revocation | **RESEARCH** | `crates/node/src/state/identity_exec.rs` | Authorised by the *old* key. No new proof system: ML-DSA-65 is already a lattice signature and every transaction carries one |
| Selective disclosure credentials | **RESEARCH** | `crates/zk-stark/src/credential.rs` | Issuer anchors a Poseidon2 root on chain, signed with the hybrid pair and verified **natively** by consensus; the holder proves membership, a predicate and an unset revocation bit in-circuit. Verifying ML-DSA inside a circuit would be 10^7–10^8 constraints, so the issuer's signature never enters one. The holder's proof is a STARK (ADR-008), so this half is post-quantum too |
| Hash-lock HTLC atomic swaps | **SHIPPED** | `crates/htlc-lattice/src/lock.rs`, `htlc-watcher`, `crates/node/src/state/htlc_exec.rs` | ADR-012. SHA3-256, BLAKE3 and SHA-256 locks on 32-byte preimages, live from genesis (`HASH_LOCK_ACTIVATION_HEIGHT = 0`). Already post-quantum: Grover needs ~2^128 sequential evaluations against a 256-bit digest. SHA-256 is the one Bitcoin and Ethereum can check, so it is the one that swaps off-chain |
| Lattice HTLC-L atomic swaps | **RESEARCH** | `htlc-lattice`, `htlc-watcher`, `crates/node/src/state/htlc_exec.rs` | `HTLC_L_ACTIVATION_HEIGHT = u64::MAX`. The lock is a Module-LWE instance `t = A·s + e` under ML-DSA-65's parameters; a claim reveals the short `(s, e)` before `expiry_height`. **Maya2C↔Maya2C only**: an atomic swap needs both chains to check the same predicate, and Bitcoin cannot check this one. Claims are never gated by the circuit breaker, because a halted claim beside a live refund is theft — [htlc-lattice.md](htlc-lattice.md) |
| Physical QKD, KM-API interface | **PLANNED** | — | ETSI GS QKD 014-style key-management interface to external QKD hardware. No code |
| Signature-suite registry (`0x01` Ed25519, `0x10`/`0x11` ML-DSA-65/87, `0x20`/`0x21` SLH-DSA-SHA2-128s/SHAKE-256f, `0x30` hybrid) | **REAL** | `crates/crypto-pq/src/suite/` | ADR-007. Closed `SuiteId` enum, exact per-suite sizes, NIST ACVP vectors for every FIPS set. `0x30` is today's hybrid byte for byte. Live from genesis (ADR-013): consensus verifies under `crypto::suites::verification_policy()`, the genesis schedule under mainnet rules, with no state read |
| Suite-tagged envelope and crypto-agility engine | **RESEARCH** | `crates/crypto-pq/src/envelope.rs`, `crates/crypto-pq/src/agility/` | ADR-007. Governance-chosen default suite (ML-DSA-87 at genesis), security-level audit (< 128 PQ bits flagged), deprecation and emergency windows, bounded per-block sweep into a vault that only a pre-registered upgraded key opens. Accounts that registered no upgraded key stay locked: seed-proof recovery is PLANNED |
| KEM suites: ML-KEM-768/1024, HQC-128/256 (**draft**), DualKem, X-Wing | **RESEARCH** | `crates/crypto-pq/src/kem_suite/` | ADR-009. DualKem = SHA3-256 over both secrets, both ciphertexts and both keys. ML-KEM pinned to NIST ACVP, HQC to the reference KATs, X-Wing to the draft vectors. Nothing on the wire uses them until Master Prompt 7 |
| Entropy HAL: OS, RDSEED, SP 800-90B health tests, SP 800-90A HMAC-DRBG | **RESEARCH** | `hal/entropy` | ADR-010. DRBG passes all 480 NIST CAVP SHA-256 cases; health-test cutoffs equal SP 800-90B Table 2; a pool refuses to seed without a healthy OS source and 256 claimed bits; a tamper trip wipes registered key stores and shuts the pool. Thermal, micro-voltage, Brownian and homodyne-QRNG sources are **SIM**; Casimir cavity is **RESEARCH** (a type that always reports unavailable). No wallet or node calls it yet |
| Transparent STARKs (Plonky3) | **SHIPPED** | `crates/zk-stark` | ADR-008. Hiding FRI + Keccak commitments over BabyBear, Poseidon2 AIRs with the standard constants. Gadgets: range proof, Merkle path (set membership), key knowledge bound to a message. Shielded pool: one joinsplit AIR for shield / transfer / unshield with a consistent-lie negative for every constraint, rejected by the verifier in release builds; also credential disclosure and sanctions non-membership. `tests/pqc_zk_tests.rs` fails if Groth16, BN254, BLS12-381 or halo2 re-enter the graph |
| Forward-secure archival keys and SLH-DSA seals | **RESEARCH** | `crates/archive/src/seal.rs` | SLH-DSA-SHAKE-256f seals over archive roots. Each epoch's seed is SHAKE256 of the last and is erased on evolve; each key certifies its successor, so one genesis key verifies the whole history. 100-epoch test, and a compromise at epoch 50 that cannot backdate. Re-sealing plan: [resealing.md](resealing.md). Not yet called by the node's archive task |

## 2. Consensus and execution

| Component | Status | Where | Notes |
|---|---|---|---|
| Hybrid-signature transaction validation | **SHIPPED** | `crates/node/src/` | Both halves verify or the transaction fails |
| Ethash-style DAG proof of work | **SHIPPED** | `crates/node/src/crypto/dag/` | With an `activation_height` registry — [dag-pow.md](dag-pow.md) |
| Committed transaction root + state root | **SHIPPED** | `crates/node/src/state/`, `crates/node/src/chain.rs` | `BlockHeader::tx_root`, checked before anything is stored — invariant 24 |
| Undo journal and reorg safety | **SHIPPED** | `crates/node/src/state/` | Every consensus record's prior value is journalled — invariants 8, 25, 26 |
| WASM runtime via Cranelift JIT | **SHIPPED** | `vm` | Gas is wasmtime fuel; native host work is charged first — invariant 21. Compiled modules are cached, keyed by bytecode *and* a digest of the engine configuration, so a config change invalidates every entry rather than serving code built under the old compiler. Measured 14x on a working contract, 23x on a trivial one — [docs/vm-module-cache.md](docs/vm-module-cache.md) |
| Historical pruning + archive bootstrap | **SHIPPED** | `archive`, `crates/node/src/state/blocks.rs` | No body deleted before a verified copy exists — invariant 27, [pruning.md](pruning.md) |
| Constant-product DEX + order book | **SHIPPED** | `dex` | Dependency-free for Kani. A losing trade is a no-op — invariants 6, 7 |
| Governance lifecycle and bounds | **SHIPPED** | `governance` | Governance cannot make governance unsafe — invariants 12, 13, 17 |
| EIP-1559 base fee over bytes | **RESEARCH** | `fee-market` | `FeeConfig::DISABLED`, activation `u64::MAX`. Called by nothing in `crates/node/src/` |
| Neural base-fee gain | **RESEARCH** | `crates/fee-market/src/model/`, `crates/node/src/neural_gas/`, `neural-gas-trainer` | A 6-16-1 integer network, i16 weights compiled in, scales EIP-1559's step by a one-sided gain — a rise ×1 to ×2, a fall ×0 to ×1 — so no feature a block producer writes can price a block below EIP-1559. Native inference, **no zkML**: every validator can re-run 112 multiplies, and a proof would cost ~10⁴× that. Trained on a synthetic demand simulator, because no chain history exists — [neural-gas.md](neural-gas.md) |
| Multi-shard asynchronous DAG (Narwhal/Tusk) | **RESEARCH** | `blockgraph` | Batch references and deterministic shard scheduling. Nothing in consensus references a batch yet — [blockgraph.md](blockgraph.md) |
| Elastic shard auto-scaling | **RESEARCH** | `crates/blockgraph/src/shard_manager/` | A per-node shard map: bisect a range above 80% of a lane for 100 ticks, buddy-merge idle pairs, 4 to 64 leaves. Not consensus — waves reach the serial state under every tiling, so lane capacity and memory pressure may drive it; the fixed `shard_of` stays for fee features. The "teleportation" is an atomic handoff of in-memory caches, with no zero-knowledge proof because nothing leaves the process — [blockgraph.md](blockgraph.md#elastic-shards) |
| Lattice Proof-of-Useful-Work (SVP solver) | **RESEARCH** | `lattice-pow` | Verification only, dependency-free. The open question is in [lattice-pow.md](lattice-pow.md) |
| On-chain zkML ONNX inference | **PLANNED** | — | Retired 2026-09-21 (ADR-008): the halo2/KZG system is gone from the workspace; the VM's `host_verify_zkml_proof` answers *no verifier*. [zkml.md](zkml.md) keeps the record |
| Stateless transfer verification | **RESEARCH** | `stateless-core`, `crates/node/src/state/stateless.rs`, `crates/light-client/src/stateless.rs` | `STATELESS_ACTIVATION_HEIGHT = u64::MAX`. From activation the accounts root is a keyed sparse Merkle tree, so a node holding only a root checks transfer blocks from witnesses. BLAKE3 commits (~0.7 KB a key); a Ring-SIS backend over `Z_q[x]/(x^256+1)` is built and measured (448 B a node) but does not commit, because no transparent lattice commitment opens in under a kilobyte. Only transfer-only blocks on a state with no sealed, oracle or governance records are decidable; no gossip topic carries witnesses — [stateless.md](stateless.md) |
| History compaction (MMR accumulator) | **RESEARCH** | `history-compactor` | A Merkle Mountain Range over one leaf per block (height, id, tx root): a pruned node keeps ≤ 64 peaks (224 B at 1M blocks) and checks any old transaction an untrusted archive serves with a ~0.8 KB proof. An accumulator, **not a validity proof**: the recursive-STARK half of Master Prompt 3 §6 is not started. Nothing in the node calls it yet |
| DAG-BFT ordering (Narwhal + Bullshark) and consensus modes | **RESEARCH** | `dag-bft` | ADR-015: DAG-BFT orders, work never does; the mode is a genesis parameter, not `config.toml`. Sans-IO engine run under `maya-sim` — 5 validators over a lossy link, crash, partition/heal, no-quorum halt. Not the node's main loop: the node still runs ArgonBlake PoW |
| WASM execution tiers (Cranelift, Pulley) | **RESEARCH** | `crates/vm/src/tier.rs` | Same deterministic config compiled natively or to Pulley bytecode; gas identical across tiers and cache hits on the whole differential corpus plus `token_swap`. Pulley measured 19x slower here, so the node keeps Cranelift + module cache |
| Bitcoin SPV header tracker | **RESEARCH** | `btc-spv` | Header rules bit-exact with Bitcoin Core (compact targets, 2016-block retarget with 4x clamp, median time past), most-chainwork fork choice, reorgs, merkle proofs refusing 64-byte and duplicate-node tricks. Verifies real mainnet blocks 0-2; a 6-block reorg test shows depth is policy, not proof. No wall-clock rule, no lock/mint path yet |
| Workload generator | **RESEARCH** | `loadgen` | Seeded transaction mixes (transfers, token transfers, contract calls, deploys) with contention over shared hot keys and Zipfian account access; `maya2c-loadgen`. Programs over a key-value state, not signed transactions — signature cost is measured separately |
| Parallel execution | **RESEARCH** | `parallel-exec` | ADR-017. Optimistic execution with in-order value validation (stale reads re-executed inline) and a declared-access-list wave scheduler; both byte-identical to sequential on 100,000 random blocks, gas independent of threads. Not full Block-STM; not in the node |
| Native smart accounts | **RESEARCH** | `smart-account` | Keys registered once (an ML-DSA-65 transfer is 3,465 B instead of 5,417 B), rotation with a stable address, validation cost bounded before any verify, daily/per-recipient/allow-list/delay policies, scoped expiring session keys, guardian recovery with a cancel window, vault delays, native/token/sponsored fees with a guaranteed maximum. ADR-016 makes it launch-blocking; not a node transaction type yet |
| Data availability (2D Reed-Solomon + sampling) | **RESEARCH** | `da` | `k × k` → `2k × 2k` Reed–Solomon with BLAKE3 row/column Merkle roots (no KZG); iterative repair detects incorrect extension; sampling clients catch a 30% withholding at the theoretical rate (99.70% measured vs 99.67% for 16 samples). No gossip message for bad-encoding proofs yet; nothing in consensus uses it |
| State sync with per-chunk verification | **RESEARCH** | `state-sync` | Snapshot chunks under a BLAKE3 Merkle root verified on arrival, parallel download from many peers, a peer banned on its first bad chunk and its outstanding chunks re-queued. Simulated at 1M accounts in tests and 100M accounts in `state-sync-sim`. The node does not yet commit a snapshot root in its header; its existing bootstrap checks the whole import against `state_root` |
| Protocol spec and conformance suite | **SHIPPED** | `spec/`, `spec-ref` | Rule-ID'd spec of encoding, transfers, state root, fees and header ids; vectors generated by a reference crate that shares no node code, replayed against the node (`tests/conformance.rs`) and an independent TypeScript verifier with its own BLAKE3. `cargo xtask spec-coverage` lists the consensus rules still without vectors (chain-level CON-4..8, ROOT-5, TX-4) |
| Protocol upgrade halt | **SHIPPED** | `node::upgrade` | Genesis-scheduled protocol versions; a block at a version this binary does not implement is refused with "upgrade required before height H" instead of being judged under old rules. Headers carry no version field and governance cannot yet move the schedule |
| Hash migration | **RESEARCH** | rehearsal test | Re-commitment of state under a new hash at an epoch boundary with a dual-hash bridge record, rehearsed on real node state; the node implements no second hash |
| Remote validator signer | **RESEARCH** | `signer`, `maya2c-signer` | Encrypted keystore, fsync'd slashing protection (refuses conflicting and below-watermark rounds, fails closed on a torn record), EIP-3076-style interchange, pinned ML-KEM/ML-DSA channel. Nothing in the node calls it: consensus signing is not wired (ADR-015). HSM/KMS backends refuse with the reason (ADR-022) |
| DoS guard | **RESEARCH** | `dos-guard` | Stateless handshake cookies (3.9M rejects/s vs 22k KEM accepts/s), puzzles, per-IP/subnet budgets, per-peer cost ledger, fee-per-byte admission, inbound diversity; simulated against spam, handshake floods, slow-loris and eclipse. Not in the node's libp2p stack yet |
| Treasury and vesting rules | **RESEARCH** | `treasury` | Per-epoch spending limit, ordered approval stages, public ledger; vesting with cliff, linear release, termination and opt-in clawback. Not wired: the genesis treasury has no spending path yet |
| Exchange deposit watcher | **SHIPPED** (tool) | `deposit-watcher` | Exactly-once crediting of finalized deposits across SIGKILLs (100,090 deposits, 40 kills). Reads a block-source trait; the node-RPC source is not written |
| Universal EVM / SVM / Move transpilation | **PLANNED** | — | Foreign bytecode lowered to the WASM runtime. No code. The hard part is not the lowering — it is that every source VM has its own gas semantics, and a transpiled contract must be priced by the fuel meter without inheriting them |

## 3. Kernel, networking and transport

The node speaks libp2p over a post-quantum Noise handshake today. Everything
below that line is intent.

| Component | Status | Where | Notes |
|---|---|---|---|
| libp2p transport, PQ Noise handshake | **SHIPPED** | `crates/node/src/network/` | ML-KEM-768; HQC available as the second KEM |
| Byzantine peer guard | **SHIPPED** | `crates/node/src/network/peer_health.rs`, `sync.rs`, `node/guard.rs` | Gossip validated before it is forwarded; gossipsub P4 scoring only; an expiring, escalating quarantine (blacklist + block list + disconnect) on attributable offences; 2 connections per peer, 256 total; bounded parent fetch. Latency recorded, never scored; no double-proposal metric, because a PoW block has no proposer. Node-local — [peer-health.md](peer-health.md) |
| Stratum V2 pool protocol | **SHIPPED** | `stratum-v2`, `pool-service` | No chain dependency in the protocol crate, so it fuzzes alone |
| SPV light client | **SHIPPED** | `light-client` | Header fork choice and state-proof verification |
| Network simulation harness | **SHIPPED** | `crates/node/src/network/sim.rs` | Latency, packet loss and partition modelling. The loss dial arrived with the radio transport: a transport whose whole problem is erasure cannot be tested by a harness that models none |
| Deterministic simulation harness | **SHIPPED** | `sim` | Seeded, virtual-time simulation: one RNG stream per model, a clock that moves only when the scheduler moves it, a network that loses, delays, reorders and partitions, and a disk that writes slowly, corruptly, half or not at all. Every failure prints the seed that reproduces it. **Dependency-free**, because a dependency is a second source of decisions and any one of them makes a replay a different run. Distinct from the row above, which is the node's own gossip model and stays where it is — [adr/ADR-006-simulation-harness.md](adr/ADR-006-simulation-harness.md) |
| eBPF/XDP relay accelerator (`aya`) | **RESEARCH** | `ebpf-net`, `hal/ebpf-net/programs`, `crates/node/src/network/relay_key.rs`, `crates/node/src/network/node/relay.rs` | A UDP block relay beside gossip, because gossip is ciphertext no kernel program can inspect. Its datagrams carry a fixed 56-byte header the XDP program reads: blocklisted sources (peer-guard quarantines only), sources over a token bucket, and structurally wrong datagrams are dropped at the driver; the rest go to AF_XDP; all other traffic passes. Chunks are XChaCha20-Poly1305 under per-peer keys agreed over the post-quantum connection. Nothing in the node binary enables it; the kernel path is Linux behind the `xdp` feature. Not measured on a native-XDP NIC — [ebpf-net.md](ebpf-net.md) |
| LoRa off-grid transport | **RESEARCH** | `radio-transport` | ISM-band header relay for regions with no IP transit. Carries **headers and SPV proofs only**: a hybrid signature is 11,165 bytes and incompressible, so a transaction is 60 frames and forty minutes of duty cycle at best. Chain-free, so its frame decoder fuzzes alone |
| Fountain-coded fragmentation | **RESEARCH** | `hal/radio-transport/src/fountain.rs` | Rateless erasure coding over a window of headers. A 1% duty cycle makes retransmission cost another window, so loss is answered by emitting more symbols rather than by asking again — there is no reverse path to ask on |
| Store-and-forward mesh relay | **RESEARCH** | `hal/radio-transport/src/relay.rs` | Custody, TTL and replay-safe dedup, until a node with IP transit is reached. A relayed header takes the identical path to one off TCP — see the note below, which this subsystem is the first real test of |
| Satellite uplink | **PLANNED** | — | Same fragmenter, a different link MTU and no duty cycle. Deferred until the terrestrial path is real |
| LEO free-space laser mesh | **PLANNED** | — | Optical inter-satellite links |
| CCSDS delay-tolerant networking (BPv7) | **PLANNED** | — | Bundle Protocol v7 store-and-forward, for links where round-trip time exceeds any sane timeout |
| Transport HAL models (SIM) | **RESEARCH** | `link-sim` | One `Link` trait (MTU, latency, loss, bandwidth, cost, class) over REAL profiles and SIM models: FSO with a 15 dB fade margin and RF fallback, Ku-band rain fade, subsea acoustic, OAM with phase-front recovery, Werner-state repeater chains with a F > 0.95 gate, orbital light time / line of sight / Doppler. QKD is mixed into the PQ secret, dropped at QBER ≥ 11%, and entanglement yields no key without a classical transcript. Tests drive DAG-BFT across an Earth–Mars 3–22 min link. Policy models, not physics simulators |
| Timing service | **RESEARCH** | `timing` | Marzullo fusion of NTP/PTP/GNSS/atomic/pulsar intervals (outliers flagged, no majority = time unknown), SR+GR rate offsets (GPS +38.5 µs/day reproduced), and the integer `within_drift` check — the only time rule a consensus path may use. Sub-picosecond network agreement is stated as unachievable |
| Subsea acoustic signalling | **PLANNED** | — | No modem code. A SIM latency/loss model (1,500 m/s) exists in `link-sim` for routing policy only |
| Subterranean neutrino signalling | **PLANNED** | — | No code that signals. `link-sim` has a RESEARCH model at the one demonstrated rate (~0.1 bit/s, MINERvA 2012) and a repetition decoder |

A note on the transports above: none of them may change consensus. A block is a
block regardless of the medium that carried it, and the chain must never
acquire a rule that depends on *how* a message arrived — that would make the
transport layer consensus-critical and hand an attacker a fork by radio.

## 4. AI and hardware integration

| Component | Status | Where | Notes |
|---|---|---|---|
| zkML proof verification (halo2, BN254) | **PLANNED** | — | Retired 2026-09-21 (ADR-008): the halo2/KZG system is gone from the workspace; the VM's `host_verify_zkml_proof` answers *no verifier*. [zkml.md](zkml.md) keeps the record |
| ONNX import, key generation, proving | **PLANNED** | — | Retired 2026-09-21 (ADR-008): the halo2/KZG system is gone from the workspace; the VM's `host_verify_zkml_proof` answers *no verifier*. [zkml.md](zkml.md) keeps the record |
| GPU mining (CUDA) | **SHIPPED** | `cuda-miner` | `cuda` feature off by default — invariant 3 |
| GPU mining (wgpu: Vulkan/Metal/DX12) | **SHIPPED** | `wgpu-miner` | [wgpu-miner.md](wgpu-miner.md) |
| Confidential federated training | **RESEARCH** | `confidential-ai` | Secure aggregation (pairwise masks from ML-KEM-768, Shamir-recovered self-masks) and integer discrete-Gaussian DP with zCDP accounting. Off-chain; writes no state. **Enclaves are a hook, not the root of trust:** a TEE attestation is a *vendor's* ECDSA signature — a trusted party (invariant-11-shaped) and not post-quantum — so privacy never depends on it, and no SGX / SEV-SNP report is generated or verified yet (no hardware, no recorded vectors) — [confidential-ai.md](confidential-ai.md) |
| IoT anchor: PUF / TPM-sealed device identity (was "TPM 2.0 / PUF hardware attestation") | **RESEARCH** | `iot-anchor`, `iot-firmware`, `crates/node/src/state/iot_exec.rs` | `IOT_ACTIVATION_HEIGHT = u64::MAX`. Device keys are ML-DSA-65 alone (no lightweight PQ signature exists; invariant 4; SLH-DSA is out of reach of a microcontroller). A fuzzy extractor turns a noisy PUF response into the seed; a TPM only seals it to PCR state and never signs, and no vendor attestation certificate is verified (a classical trusted party, invariant 11). Enrollment proves key possession and owner binding, not genuine silicon. Devices sign Merkle-rooted telemetry batches; bounds are flagged, never refused; signed tamper events and conflicting batches retire a device. `no_std`, heap-free; 258,532 bytes of stack measured under QEMU (Cortex-M33) — [iot-anchor.md](iot-anchor.md) |
| Biomolecular TRNG | **PLANNED** | — | Entropy source. Would feed the beacon alongside the VRF, never replace it |
| Photonic optical tensor driver | **PLANNED** | — | |
| DNA quaternary archival engine | **PLANNED** | — | Cold storage tier beneath `archive`'s CAR v1 batches |
| Magneto-optical MRAM driver | **PLANNED** | — | |
| Bio-silicon neural organoid MEA interface | **PLANNED** | — | |

None of the PLANNED hardware above may sit on the consensus path without a
determinism argument. A driver that returns a different value on two machines
is a fork; §7 states the rule.

## 5. Security and macro engines

| Component | Status | Where | Notes |
|---|---|---|---|
| Fuzz targets (cargo-fuzz, libFuzzer) | **SHIPPED** | `fuzz/` | Decoders and SV2 frames; `car_decode.rs` fuzzes untrusted archives |
| Kani model checking | **SHIPPED** | `ledger-math`, `dex`, `governance`, `fee-market`, `threat-intel` | Which is why those crates stay dependency-free — invariants 1, 6. `threat-intel`'s 4 harnesses verified 2026-09-15 (Kani 0.67.0, Linux) |
| Supply-chain gates | **SHIPPED** | `deny.toml`, `cargo audit` | Committed, run in CI |
| Telemetry threat surface | **SHIPPED** | `telemetry` | Nothing on the dashboard is verified, and that is stated rather than papered over — invariants 14, 15 |
| State invariant guard + circuit breaker | **SHIPPED** | `crates/node/src/state/invariant_guard/` | Value conservation over all five holding places fails the block; an anomaly halts one module for 100 blocks and never a transfer — invariant 28, [invariant-guard.md](invariant-guard.md) |
| Exploit replay suite | **SHIPPED** | `crates/node/tests/exploit_replays.rs` | Re-entrancy, overflow and flash-loan replays. Two of the three have no surface on this chain, which the suite demonstrates rather than assumes |
| Agent-based economic simulator (SIM) | **RESEARCH** | `econ` | Master Prompts 6 §6 and 18. Calls `fee-market`'s base-fee step and split and `dex`'s curve and batch auction, so it cannot drift from them. Scenarios, macro shocks, cost-of-attack, Nakamoto/Gini, sandwich extraction, fee-quote accuracy. Prices are inputs, never forecasts; no module guarantees price stability — [ECONOMIC_SECURITY.md](ECONOMIC_SECURITY.md) |
| LibAFL dynamic fuzzer | **RESEARCH** | `offsec-sandbox`, `crates/node/tests/fuzz_harness.rs` | A coverage-guided, structure-aware harness beside the `fuzz/` libFuzzer targets, sharing their corpora. Structure-aware mutators for transactions, WASM modules and handshakes hunt panics, unbounded allocation and non-determinism — not "memory corruption" (safe Rust) or "races" (the apply path is single-threaded; the real property is deterministic re-execution). Crash triage emits regression-test stubs, never patches (invariant 13). A separate workspace, so LibAFL and an optional Z3 never touch the node graph or Kani. The in-tree gate replays seeded mutations through the decoders and apply path — [offsec-sandbox.md](offsec-sandbox.md) |
| Threat-intel registry (was "ZK-SIEM threat mesh") | **RESEARCH** | `threat-intel`, `threat-firewall`, `crates/node/src/state/threat_exec.rs`, `crates/node/src/network/evidence_tap.rs` | `THREAT_INTEL_ACTIVATION_HEIGHT = u64::MAX`. An indicator is an ed25519 gossip author's own gossipsub signature over a frame that decodes and fails a stateless check — a hybrid signature or a `tx_root` — re-checked by every node at apply. No ZK (the evidence hides nothing), no votes (no validator set; one verified offence confirms), no IP on chain (no proof binds one to a key; each node maps authors to its own connections). Floods and scans leave nothing a third party can verify and get no indicator. Score halves every 720 blocks; the per-host `threat-firewall` worker turns active indicators into nftables / XDP blocks. The evidence signature is classical — [threat-intel.md](threat-intel.md) |
| Self-synthesizing bytecode hot-patcher | **PLANNED** | — | **Reconcile with invariant 13 before any code is written.** No governance key's value is a program, and native code is never fetched from chain state and run. A hot-patcher that takes its patch from the chain violates that outright; one that selects between implementations the binary already ships does not |
| ISO 20022 XML messaging parser | **RESEARCH** | `iso20022` | Bank-rail interoperability: pacs.008, pacs.009, camt.053. Chain-free like `stratum-v2`, so the decoder of untrusted XML fuzzes alone — `fuzz/fuzz_targets/iso20022_decode.rs`, landed with the crate rather than after it. Entity expansion is off: XXE and entity-expansion bombs are the class a bank-rail parser meets first |
| ISO 20022 → L1 bridge | **RESEARCH** | `crates/iso20022/src/bridge.rs`, `api-gateway` | Translates a payment instruction into a sealed `TxKind` and renders camt.053 back out of committed state. Refuses mainnet, the way `state::zkml::check_setup` does, because the envelope's confidentiality is classical and envelopes are on chain forever. The gateway signs for payments that arrive with no Maya2C key, which makes it the chain's second trusted party after the oracle — absent by default |
| Sanctions non-membership proofs | **RESEARCH** | `crates/zk-stark/src/sanctions.rs`, `crates/iso20022/src/sanctions.rs` | Proves a party is *not* on a published list without revealing who they are, so a compliance check costs no anonymity. The AIR sits in `zk-stark`, which already owns the field, the Poseidon2 hash and the tree; `iso20022` holds only the identifier encoding, so its XML decoder never pulls a prover into a fuzz target |
| Real-world asset primitives | **RESEARCH** | `rwa`, `crates/node/src/state/rwa.rs` | `RwaToken`, a paged `OwnershipCapTable`, `LegalAttestation`, `RevenueDistribution`. Chain-free records; the cap table is paged like the revocation bitmap because a single unbounded one is a record nobody wrote a limit for |
| Atomic delivery-versus-payment | **RESEARCH** | `crates/node/src/state/rwa_exec.rs` | Both legs or neither — and a settlement that cannot complete is a **no-op**, never an `Err`. Invariant 7: a failing transaction fails its whole block here, so a DvP that errored would hand anyone a way to void a block |
| Jurisdictional transfer rules | **RESEARCH** | `crates/node/src/state/rwa_exec.rs` | An issuer names a claim schema, a predicate and trusted credential issuers; the binary evaluates. Invariant 13 — no governed value is a program, so an issuer selects a rule rather than supplying one. Eligibility is proved once and cached in `r:elig:`, because a Groth16 check per transfer is 10-20 seconds on a ten-thousand-transfer block |
| Pro-rata revenue distribution | **RESEARCH** | `ledger-math::distribute` | Largest remainder, tie-broken by holder index. Kani-checked that the payouts sum to exactly the total — a rounding rule that lost a base unit would be refused by the invariant guard, so it is a correctness property rather than a fairness one |
| ZK dark pools | **PLANNED** | — | Would build on `dex` and `zk-stark`. Invariant 7 applies: a trade that merely loses is a no-op |
| Relativistic gravitational clock synchronisation | **PLANNED** | — | **Note invariant 9.** Oracle freshness is measured in block height, never in timestamps, because `header.timestamp` is miner-chosen and unbounded. A better clock does not change that — it would have to arrive with a rule bounding what a miner may write |

## 6. Tooling and formal verification

| Component | Status | Where | Notes |
|---|---|---|---|
| Tauri 2.0 desktop wallet | **SHIPPED** | `apps/wallet-gui/` | Plus the CLI wallet in `wallet` |
| Leptos block explorer and dashboard | **SHIPPED** | `explorer`, `apps/dashboard/` | `apps/dashboard/` is not a workspace member — CSR Leptos is `wasm32` only |
| Axum REST/GraphQL gateway | **SHIPPED** | `api-gateway` | Talks JSON-RPC to a node, never opens the state database |
| Multi-language SDKs | **SHIPPED** | `sdk-ffi`, `sdk-wasm`, `sdk-js` | Which bindings are generated but never compiled: [sdk.md](sdk.md) |
| LaTeX technical reference generator | **SHIPPED** | `docgen` | Generated from module documentation |
| Testnet faucet | **SHIPPED** | `faucet` | Two independent rate-limit buckets — invariant 16 |
| Workspace automation (`xtask`) | **SHIPPED** | `xtask` | `cargo xtask disk` measures build output against the 30 GiB ceiling — this volume has been filled to zero bytes twice, and a full disk reports as `os error 112`. `cargo xtask coverage` fails when `features.toml` claims a subsystem works and names no test that exists |
| Lean 4 mathematical proof engine | **PLANNED** | — | Machine-checked proofs of the ledger and consensus rules. Kani covers the arithmetic crates today; Lean would reach what bounded model checking cannot |

## 7. Execution directives

Four standing rules. Each is stated with its *current* enforcement, because a
directive documented as achieved is a directive nobody will implement.

**Zeroization of cryptographic memory.** `zeroize` is a dependency of every
crate that holds key material: root, `crypto-pq`, `custody-mpc`, `mev`, `vrf`,
`wallet`, `apps/wallet-gui/core`, `apps/wallet-gui/src-tauri`, `app-maya2c`. New secret
types get `ZeroizeOnDrop`, not a manual `drop`. The gap is that a `Vec<u8>` that
has passed through a serializer has already been copied; the rule is to keep
secrets in typed wrappers from generation to use, never in a raw buffer.

**Deterministic execution across WASM and state updates.** Enforced by two
mechanisms rather than by convention: `apply_block_journaled` refuses a
`state_root` that execution does not reproduce (invariant 24), and no
floating-point value may enter a consensus rule (invariant 20 — the reason
`tract-onnx` is confined to `zkml-prover`). Anything new on the consensus path
must be reproducible bit-for-bit on a different machine and a different
toolchain, or it is not a consensus rule.

**No dynamic heap allocation inside critical consensus loops.** A target, not a
description. The apply path allocates today — RocksDB's interface is owned
buffers, and the journal collects into `Vec`. Read it as: do not add an
allocation to an inner loop that did not have one, prefer reusing a buffer
across iterations, and measure before calling a loop hot.

**No `unsafe` in core execution paths.** Holds for `crates/node/src/state/`, `crates/node/src/chain.rs`,
`ledger-math`, `dex`, `governance`, `fee-market`, `vrf` — the crates whose
dependency-freedom exists precisely so they can be model-checked. These carry
`unsafe` by construction and are exempt: `cuda-miner` (FFI to the CUDA driver),
`sdk-ffi` (the C ABI is the product), `wgpu-miner` (GPU buffer mapping),
`hal/ebpf-net/src/linux/` (AF_XDP rings and UMEM are memory shared with the kernel;
behind the `xdp` feature, so a default node links none of it), and
`hal/ebpf-net/programs` (the XDP program reads packets through pointers the kernel
verifier bounds). An `unsafe` block anywhere else needs a hardware-attestation
justification in a comment, and a reviewer.

---

## Adding a subsystem

1. It lands here first, with a status tag and one line on what it touches.
2. It becomes a workspace member with tests — status **RESEARCH**.
3. Consensus calls it, behind an activation height — status **SHIPPED**.
4. Only once a test pins behaviour that must never change does it earn a
   numbered invariant in `CLAUDE.md`.

Skipping step 1 is how a tree acquires a crate nobody can explain. Skipping
step 4 is how `CLAUDE.md` acquires a claim nobody can check — and one
uncheckable line there costs more than the subsystem, because it makes the
other twenty-seven need re-verification.
