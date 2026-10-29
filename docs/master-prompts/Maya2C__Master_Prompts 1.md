## MASTER PROMPT 1: Foundation (Workspace, Memory, Build Hygiene, Reality Ledger)

```
/plan Act as Maya2C's Principal Architect and Rust Build Engineer. Build the foundation every later phase depends on. Do NOT build product features in this phase.

CONTEXT
Maya2C is a post-quantum Layer-1 ecosystem written in Rust (nightly). Node crate: custom-l1-node (binary: maya2c-node). Scope: cryptography, state, DAG consensus, WASM VM, DeFi/RWA, multi-transport networking, security tooling, clients, deployment. Master Prompts 2–10 build those. Your job now: make the workspace clean, fast, honest, and self-documenting.

1. AUDIT FIRST
- Map the repo: every crate, binary, test, bench, script. Mark which compile. Save as reports/00-inventory.md.
- Measure target/ size, then run cargo clean.
- Find duplicates (two state_pruner crates, several governance modules, three explorers, many genesis binaries). Plan merges. Do not delete work: move superseded code to attic/ with a README saying what replaced it.

2. WORKSPACE LAYOUT
- One root Cargo.toml workspace: resolver = "3", edition = "2024", [workspace.dependencies] with ONE version per dependency, shared [workspace.lints] (unsafe_op_in_unsafe_fn = deny, clippy::unwrap_used = deny outside tests, clippy::pedantic = warn).
- Folders:
  crates/   libraries (types, crypto, zk, state, storage, fees, consensus, dag, mempool, vm, p2p, ...)
  bins/     maya2c-node, maya2c-miner, maya2c-gpu-miner, l1-wallet, maya2c-cli, genesis-ceremony
  apps/     explorer, wallet-gui, portal, faucet, dev-hub, ledger-maya2c
  hal/      hardware abstraction layers + simulators
  sdks/     ts, python, go, uniffi
  formal/   Lean 4, Kani, Aeneas
  infra/    terraform, ansible, helm, docker, grafana, systemd
  sim/, fuzz/, benches/, tests/, xtask/, docs/, reports/, attic/
- Cargo feature tiers: core (default: crypto, state, consensus, vm, rpc), extended (DeFi, RWA, identity, bridges, AI), frontier (all hardware / space / bio / quantum HAL simulators). A default build compiles core only.
- rust-toolchain.toml: nightly pinned to a dated nightly (stays bleeding-edge but reproducible). Components: rustfmt, clippy, miri, rust-src, llvm-tools.

3. BUILD SPEED + DISK (fixes the 415 GB target folder)
- [profile.dev]: debug = "line-tables-only", split-debuginfo = "unpacked", incremental = true.
- [profile.dev.package."*"]: opt-level = 3, debug = false.
- [profile.release]: lto = "fat", codegen-units = 1, strip = true, panic = "abort" for binaries.
- [profile.ci]: inherits dev, debug = false.
- .cargo/config.toml: mold linker on Linux, lld on Windows; sccache if installed. Use cargo-nextest for tests.
- xtask: `cargo xtask disk` reports target size and warns above 30 GB.

4. REALITY LEDGER: features.toml
- Register every feature from old prompts #1–#162 plus Benchmark and Memory. Fields: id, name, crate, tier, class (REAL / SIM / RESEARCH), test files, status (planned / stub / working / verified).
- `cargo xtask coverage` prints the full table. CI fails if a feature marked working has no passing test.

5. DETERMINISTIC SIMULATION HARNESS (sim/)
- Seeded, deterministic multi-node simulator: fake clock, fake network (latency, loss, partitions, reordering), fake disk (corruption, slow writes). Model it on FoundationDB / TigerBeetle simulation testing; evaluate madsim or turmoil.
- Every failure prints its seed so it can be replayed exactly.
- All later chaos, latency, partition, Earth–Mars, subsea, and Byzantine tests use this harness.

6. PROJECT MEMORY
- CLAUDE.md (≤ 300 lines): mission, architecture map, crate boundaries, tiers, reality classes, code conventions, test commands, Standing Orders (below), how to resume.
- docs/adr/: one Architecture Decision Record per major choice. Write ADR-001 (workspace layout) now.
- PROGRESS.md: checklist of Master Prompts 1–10 and their sub-steps.

7. STANDING ORDERS (copy into CLAUDE.md; they apply to every future task)
- Never claim a test passed, a benchmark number, or "0 warnings" unless the real command output is pasted into the report.
- Numbers in prompts (1M TPS, 10x faster, sub-10 ms) are TARGETS. Measure and report real numbers. Never fake or hard-code them.
- SIM and RESEARCH modules show their class in logs, docs, and CLI output. No simulator can reach a production code path without an explicit feature flag.
- Consensus-critical code: no floats, no HashMap iteration order, no wall-clock time, no heap allocation in hot loops, no unsafe without a // SAFETY: comment and a test.
- All secret-key types use zeroize + secrecy. Never log secrets.
- Never run anything that costs money, touches live servers, or publishes packages (terraform apply, deploy scripts, cargo publish, npm publish) without asking me first.
- Work in small steps. After each step: cargo check, run tests, commit with a clear message, tick PROGRESS.md. If context runs low, stop cleanly and write where to resume.
- Every major design choice gets an ADR in docs/adr/.

8. CI SKELETON
- .github/workflows/ci.yml: fmt, clippy -D warnings, nextest (core tier), cargo-deny (licenses, advisories, duplicate versions), cargo xtask coverage.
- Nightly scheduled workflow for heavy jobs: frontier tier, fuzzing, formal proofs.

DONE WHEN
cargo check --workspace passes for core; clippy is clean for core; target/ stays under 30 GB after a full core build; features.toml lists all 164 entries; CLAUDE.md, PROGRESS.md and ADR-001 exist; reports/01-foundation.md contains real command output.
```

---

## MASTER PROMPT 2: Cryptography, Zero-Knowledge and Key Custody

```
/plan Act as Maya2C's Principal Cryptographic Engineer. Follow the CLAUDE.md Standing Orders. Build crates/crypto, crates/zk, crates/custody, hal/entropy, and apps/ledger-maya2c.

1. SIGNATURE SUITES + CRYPTO-AGILITY
- A SignatureSuite trait. Every transaction envelope carries a suite ID, so algorithms can change later without breaking old data:
  0x01 Ed25519: legacy/devnet only. Latest ed25519-dalek, verify_strict() only, zeroize, PKCS#8.
  0x10 ML-DSA-65 (FIPS 204)
  0x11 ML-DSA-87 (NIST Category 5, mainnet default)
  0x20 SLH-DSA-SHA2-128s (FIPS 205)
  0x21 SLH-DSA-SHAKE-256f (cold vaults)
  0x30 Hybrid = ML-DSA + SLH-DSA. Valid ONLY if BOTH verify.
- Prefer RustCrypto ml-dsa / slh-dsa crates; pqcrypto-* as fallback. Pin versions. Record the choice in an ADR.
- Signature fields are variable-length. Take exact sizes from the crate and the final FIPS text (ML-DSA-65 is 3,309 bytes in final FIPS 204, not the 3,293 draft size).
- Crypto-agility engine: governance can change the default suite. Each suite has a security-level entry. An audit job flags any suite below 128-bit post-quantum security. Accounts on a deprecated suite get a migration window; after that, funds move to a vault that only an upgraded key can open. Test: emergency migration of 1,000,000 simulated accounts with no downtime.
- Test with official NIST ACVP / KAT vectors, not only sign-then-verify round-trips.

2. KEM SUITES (transport wiring happens in Master Prompt 7)
- ML-KEM-768 and ML-KEM-1024 (FIPS 203). HQC-128 and HQC-256 (NIST 2025 selection; label as draft standard).
- DualKem combiner: shared secret = KDF(ML-KEM secret || HQC secret || both ciphertexts). Breaking one KEM is not enough.
- Also offer an X-Wing-style hybrid (ML-KEM + X25519), which protects against implementation bugs in new PQ code today.

3. HASHING + ArgonBlake
- ArgonBlake: BLAKE3 → Argon2id (32 MiB) → BLAKE3. Parameter struct, test vectors, SIMD through BLAKE3's AVX2 / AVX-512 / NEON backends. Used by the argonblake-pow devnet mode (Master Prompt 4).

4. SECRET MEMORY SAFETY
- zeroize-on-drop for every key type, secrecy wrappers, no Debug/Display for secrets, constant-time comparisons with subtle.
- dudect-style timing tests for signing, verification, and decapsulation.

5. ENTROPY (hal/entropy)
- EntropySource trait. REAL: OsRng, RDSEED. SIM: thermal / micro-voltage sensors, microfluidic Brownian-motion sensor, vacuum-fluctuation QRNG (real devices use optical homodyne detection; model that). The Casimir-cavity version stays RESEARCH.
- Never trust one source: mix all sources into a NIST SP 800-90A HMAC-DRBG. Run SP 800-90B health tests (repetition count, adaptive proportion) on every source, and automatically disable a source that fails.
- Offline test tool: run output through NIST SP 800-22 and Dieharder and save the results.
- A tamper event zeroizes the key stores (connects to the firmware guard in Master Prompt 8).

6. ZERO-KNOWLEDGE: TRANSPARENT FROM DAY ONE
- No Groth16, no BN254, no trusted setup anywhere in the workspace.
- Choose one proof system and record it in an ADR: hash-based STARK (Plonky3) or Binius are the post-quantum choices. Halo2-IPA is transparent but NOT post-quantum; allow it only where the ADR says so.
- Shielded pool: note commitments, nullifiers, commitment Merkle tree, mint / transfer / unshield circuits, range proofs.
- Reusable gadget library: range proof, set membership, Merkle path, knowledge of a signature key.
- Under-constraint checks: for every witness, a negative test that tampers with it and expects rejection.
- tests/pqc_zk_tests.rs: soundness negatives, plus a check that `cargo tree` has no ark-groth16 / ark-bn254.

7. THRESHOLD CUSTODY (crates/custody)
- REAL now: m-of-n ML-DSA multisig enforced by on-chain policy.
- RESEARCH (behind a feature flag): true threshold lattice signing (DKG + partial signature aggregation). The full key must never exist in one place. Enable only after a peer-reviewed scheme is chosen in an ADR.
- Shares travel over mutually authenticated PQ Noise/TLS. Test: 3-of-5 signing with simulated node failures.

8. LATTICE HTLC (htlc_lattice)
- REAL: hash-locks on SHA3-256 / BLAKE3 pre-images. At 256-bit these are already quantum-safe, because Grover's algorithm only halves the security. Document this.
- RESEARCH variant: Module-LWE commitment locks.
- Lock, claim, and refund by block height T. A counterparty watcher worker claims automatically before timeout.
- tests/htlc_lattice_tests.rs: successful swap, refund on timeout, forged pre-image rejected.

9. LONG-TERM ARCHIVAL CRYPTO
- Forward-secure key evolution (a new key each epoch, old keys erased). SLH-DSA seals for archives. A documented re-sealing plan for when algorithms age. Test over 100 simulated epochs.

10. LEDGER HARDWARE APP (apps/ledger-maya2c)
- Ledger Rust SDK. APDUs: GET_PUBLIC_KEY, SIGN_TRANSACTION (ML-DSA-65), DISPLAY_ADDRESS.
- Measure RAM use. ML-DSA signing may not fit small devices; try streaming / low-memory signing first. If it cannot fit on Nano S Plus, target Stax/Flex and document why.
- tests/ledger_tests.rs using the Speculos emulator.

11. CRYPTO BENCHMARKS
- benches/crypto.rs (criterion): keygen / sign / verify for every suite, KEM encaps/decaps, ArgonBlake, SHA-256, kHeavyHash reference, plus a key/signature size table. Export CSV and Markdown to reports/.

DONE WHEN
All REAL items pass with NIST vectors; SIM and RESEARCH items are labeled; features.toml is updated; ADRs exist for the signature, KEM, and ZK choices; reports/02-crypto.md contains real output.
```

---

## MASTER PROMPT 3: State, Storage and Economics

```
/plan Act as Maya2C's State Machine and Storage Architect. Follow the CLAUDE.md Standing Orders. Build crates/types, crates/state, crates/storage, crates/fees, crates/stateless, crates/archive.

1. CORE TYPES
- Pick an account model (not UTXO) and write an ADR.
- Transaction: suite_id, sender, nonce, fee fields (max_fee, priority_tip), payload, signature(s), public key or key hash.
- BlockHeader / DAG vertex: parents: Vec<Hash>, round, state_root, timestamp, difficulty, work solution (nonce or PoUW vector).
- One canonical, versioned, deterministic encoding for hashing (borsh or SSZ). A zero-copy wire format for speed (rkyv or flatbuffers). Golden test vectors for every type.

2. STATE DB
- StateDB over RocksDB with column families (accounts, code, storage, meta, indexes).
- Transition order: signature → nonce → balance → fees → apply. Checked arithmetic everywhere.
- Atomic block commit with WriteBatch. Full rollback if anything fails.
- Tests: double-spend, bad nonce, rollback, overflow.
- proptest: random transaction sequences never change total supply except through defined mint/burn rules.

3. STATE COMMITMENTS + STATELESS VALIDATION
- Commitment trait with backends: Sparse/Jellyfish Merkle tree (REAL, default); Verkle with IPA/KZG (REAL, labeled not post-quantum); lattice polynomial vector commitments over R_q = Z_q[x]/(x^n+1) (RESEARCH).
- Stateless mode: each transaction carries a witness (target under 1 KB); validators check against the header root with no local StateDB lookups.
- Test: a light node validates 10,000 transitions. Report measured RAM against the 10 MB goal.
- benches/stateless_bench.rs: witness size, proof time, verify time, bandwidth saved.

4. FEE MARKET + SUPPLY
- EIP-1559 base fee + priority tip + byte-length floor price.
- Split: 80% of base fee burned, 20% to DAO treasury, 100% of tip to the block producer. Hard maximum-supply invariant.
- The fee market is ACTIVE by default (no DISABLED config). It is enforced at the mempool gate and in the state machine before execution. Base-fee burn is wired into the reward engine.
- Multi-dimensional fees: separate prices for compute, storage bytes, bandwidth, and proof verification, so no single resource can be spammed cheaply.
- Tests: tests/spam_defense_tests.rs (underpaying tx rejected at mempool); total supply falls under heavy load.

5. PRUNING, ARCHIVE, RESURRECTION (old prompts #106 and #107 were identical; build once)
- Prune finalized history deeper than K epochs; keep the roots.
- Export old batches as CAR files to IPFS / Arweave (real clients, mocked in tests).
- Dormant-account expiry plus resurrection: submit an inclusion proof to restore an account.
- Tests: a pruned node bootstraps from a snapshot and validates new blocks without full history; measure disk saving (target 90%) and confirm 100% of accounts can be restored.

6. RECURSIVE HISTORY COMPACTION
- Fold history into one proof (Nova/SuperNova folding, or recursive STARK). Label curve-based folding as not post-quantum.
- On-demand extraction: verify one old transaction against the compact proof plus an archive fetch.
- benches/compactor_bench.rs: folding time and memory for 1,000,000 blocks (real numbers).

7. TIERED STORAGE (hal/storage)
- StorageTier trait:
  Hot: RAM (REAL), CXL 3.1 memory pools (SIM), spintronic MRAM as a byte-addressable tier (SIM)
  Warm: local NVMe with io_uring (REAL), NVMe-oF / RDMA (SIM)
  Cold: object storage (REAL), 5D-glass / holographic (SIM), DNA (SIM, codec below)
- NUMA-aware placement on Linux (measure the effect).
- Integrity: checksums on every block plus scrubbing, instead of any exotic "bit-flip immunity" claim.
- Tests: 64 shards writing concurrently. benches/cxl_bench.rs runs against the simulator.

8. DNA ARCHIVE CODEC (the software is REAL; the lab is SIM)
- Binary → nucleotides (2 bits per base) using a rotation code so homopolymer runs stay ≤ 3; GC-content balancing; outer + inner Reed–Solomon; index and primer addressing.
- FASTA / FASTQ output. Simulated synthesis and sequencer interfaces.
- Test: 1 MB snapshot → DNA → noise (substitutions, insertions, deletions, 15% strand loss) → lossless restore.

9. HYPERDIMENSIONAL INDEX (optional)
- 10,000-dimension binary hypervectors (bind / bundle / permute) for fuzzy search and recovering partial data.
- NEVER used for consensus; exact state stays in the commitment tree. Benchmark honestly against trie lookups.

DONE WHEN
Core state tests and property tests pass; fee market is active; pruned-node test passes; DNA round-trip passes under noise; reports/03-state.md contains real numbers.
```

---

## MASTER PROMPT 4: Consensus, Mining and Scaling

```
/plan Act as Maya2C's Consensus Architect. Follow the CLAUDE.md Standing Orders. Build crates/consensus, crates/dag, crates/mempool, crates/pouw, crates/sharding, crates/staking, bins/maya2c-miner, bins/maya2c-gpu-miner, hal/accel, hardware/hdl.

0. RESOLVE THE DESIGN FIRST (write ADR-consensus before coding)
Earlier prompts mixed PoW, PoUW, PoS staking, and DAG-BFT. Make one coherent design:
- Ordering + finality: DAG-BFT (Narwhal mempool + a Bullshark / Mysticeti-style commit rule; Tusk was the original, newer rules have lower latency), run by staked validators.
- Work (PoUW): state exactly where it enters, e.g. new-coin issuance and Sybil-resistance for proposers or compute workers.
- Pluggable ConsensusMode set in config: argonblake-pow (devnet, single chain), pouw-lattice, dag-bft (mainnet). The same state machine runs under every mode.

1. CLASSIC CHAIN MODE (argonblake-pow)
- Retarget every 100 blocks, 15 s target, max 4x change per retarget.
- Fork choice = most cumulative work. Reorg with state rollback.
- bins/maya2c-miner: multi-threaded CPU miner that fetches candidates over RPC and submits solved blocks.
- cargo test --all confirms state consistency across reorgs.

2. DAG ENGINE
- Multi-parent headers; workers broadcast transaction batches; primaries certify; the commit rule orders the DAG deterministically. Garbage-collect old rounds.
- Wire it into the maya2c-node main loop. No unreferenced crates.
- Test: 5-node cluster in sim/. Report real throughput and the bottleneck (goal: 50,000 TPS).

3. ENCRYPTED MEMPOOL (anti-front-running, anti-MEV)
- Epoch DKG among validators → clients encrypt transactions to the epoch key → order first → threshold-decrypt → execute.
- The ADR must be honest: practical threshold encryption today is pairing-based (not post-quantum). Lattice threshold decryption is RESEARCH behind a flag.
- Fallback: commit-reveal + batch auction ordering.
- Tests: payload stays hidden before commit; execution is deterministic after commit.

4. LATTICE PoUW
- Puzzle: find a short vector in a seeded lattice (approximate SVP). Difficulty comes from lattice dimension and the Hermite-factor bound. Verification must be cheap (norm check + lattice membership).
- LLL / BKZ reduction with AVX-512 / NEON.
- The validator rejects invalid vectors and vectors above the threshold (tests).
- The ADR must define what is "useful" about the work (e.g. published lattice-challenge records for cryptanalysis research). If nothing useful is produced, call it lattice PoW.

5. GPU MINER (bins/maya2c-gpu-miner)
- wgpu + WGSL shader shaders/pow_lattice.wgsl (mod-q matrix-vector multiply). Worker pool across all GPU adapters; streams headers in, solved vectors out (crossbeam-channel, tokio).
- clap flags: --devices, --workgroup-size, --rpc.
- tests/gpu_validation.rs: GPU results match the CPU reference bit-for-bit (skip cleanly with a message if no GPU).

6. ACCELERATOR HAL (hal/accel)
- MatVecAccelerator trait with backends:
  CPU AVX-512/NEON (REAL), GPU wgpu (REAL),
  FPGA (REAL HDL: hardware/hdl/zk_accelerator.v with NTT + MSM pipelines and AXI-Stream DMA, verified with Verilator + cocotb against the Rust / reference prover),
  photonic MZI / oTPU (SIM with phase-drift + thermal-noise model + calibration loop),
  neuromorphic / organoid MEA (RESEARCH SIM),
  "stellar-scale" compute (a photonic-SIM benchmark profile).
- Rule: output from any non-deterministic backend is re-verified on CPU before it can affect consensus.
- benches/accel_bench.rs across every backend; thermal-efficiency (work per watt) scheduler using RAPL readings where available.

7. SHARDING + AUTO-SCALING
- Key-range shard map. Split a shard when it runs above 80% capacity for 100 rounds; merge when idle. Rebalance without stopping block production. Locality-aware cross-shard routing.
- Design limit 2^16 shards. Tests: scale 4 → 64 → 4 in sim; 1,000 shards in sim with no cross-shard lockups.
- Cross-shard: atomic two-phase commit plus ZK burn-and-mint "teleport", with timeout rollback. Test: 50,000 cross-shard transfers across 16 shards, zero double-spends.

8. TIERED CONSENSUS + NODE ROLES + EDGE
- Tier 1 (fiber / eBPF), Tier 2 (LEO / radio), Tier 3 (acoustic / deep space). Tier 2/3 run local sub-DAGs with local finality; their roots are batched into Tier 1 at variable epochs. Test: a 10 s subsea delay does not stall Tier 1 throughput.
- Roles: Validator (retail: 4 vCPU, 8 GB) verifies succinct proofs only. Compute Worker (heavy, TEE-attested) sells PoUW / tensor work through a task auction; validators keep ordering. Test: a low-spec node fully validates a zkML-heavy block.
- Edge ingestion: edge gateways batch and prove 10,000 tx into one proof. Anti-Sybil via stake + TPM attestation. Note: STARK proofs are tens to hundreds of KB; the header stores a 32-byte commitment to the proof. Measure the real bandwidth reduction (goal 100x).
- Time rules: oracle triggers use block height / epoch windows, never wall-clock time. Fall back to local sub-DAGs when transport latency exceeds slot time. Kani proofs on the invariant module (crates/ledger-math invariant 9).

9. STAKING, SLASHING, REWARDS
- Registration, minimum PQ bond, active-set selection, delegation, epoch rewards.
- Slashing with on-chain evidence for double-signing, long downtime, and invalid proofs (bond burned).
- Test: 1,000 validators joining, leaving, and getting slashed under Byzantine faults.

DONE WHEN
All three consensus modes run in sim/; reorg and DAG commit tests pass; GPU/CPU parity passes (or skips with a reason); reports/04-consensus.md has measured TPS and latency.
```

---

## MASTER PROMPT 5: VM, Smart Contracts and On-Chain AI

```
/plan Act as Maya2C's VM and Compiler Engineer. Follow the CLAUDE.md Standing Orders. Build crates/vm, crates/contract-sdk, crates/multivm, crates/zkml, crates/neural-gas, crates/confidential-ai, crates/agents.

1. DETERMINISTIC WASM VM
- wasmtime with fuel metering, epoch interruption, memory/table/stack limits, NaN canonicalization, no threads, no non-deterministic SIMD, no WASI in consensus.
- Host functions: read account state, block height, emit events, call contract, crypto precompiles (hashing, ML-DSA verify, ZK verify).
- contract-sdk: #![no_std] Rust contracts → wasm32-unknown-unknown. `maya2c-cli contract build` runs wasm-opt and deploys through an L1 transaction.
- Example contracts: token, token swap, multisig. Test: out-of-gas rolls back cleanly.
- Gas schedule: benchmark every host function and set price from measured cost. Store it as a governance parameter.

2. EXECUTION TIERS
- wasmtime already compiles with Cranelift. Compare three tiers: Pulley interpreter (wasmtime's portable interpreter), Cranelift JIT, cached precompiled modules keyed by code hash.
- A hot-path profiler picks the tier per contract.
- Gas must be IDENTICAL in every tier. Differential tests run the same calls through all tiers.
- Criterion benchmark of the real speedup on complex contracts (goal: 10x).

3. MULTI-VM
- Phase A (REAL): host an EVM (revm) and Solana SBF (solana-sbpf) as native engines with unified state mapping and gas conversion.
- Phase B: Move VM.
- Phase C (RESEARCH): transpile EVM / SBF / Move bytecode to WASM.
- Test: an ERC-20, an SPL-token-style contract, and a Move coin run side by side in one block, atomically, with gas parity checks.

4. zkML
- ONNX inference with tract. Proofs through a zkML stack (EZKL-style on Halo2: label not post-quantum; or STARK-based).
- host_verify_zkml_proof(): report measured verify time (goal < 10 ms). Gas for tensor ops.
- ZKML_ACTIVATION_HEIGHT comes from config: 0 on devnet.
- Test: a small quantized classifier proven and verified inside an L1 block.

5. NEURAL GAS PRICING
- Consensus uses integer-only, fixed-point, quantized inference whose output is hard-clamped to EIP-1559 bounds. The model can never break the fee market.
- Features: tx size, state-access overlap, memory depth, cross-shard dependencies. Train offline on recorded sim/ data. The model hash is committed on-chain, and governance approves new models.
- benches/gas_predictor.rs: validation time stays stable under 100,000 tx/s bursts.

6. CONFIDENTIAL AI + AGENTS
- Tee HAL: verify SGX DCAP, AMD SEV-SNP, and Intel TDX quotes (REAL verification libraries; hardware is SIM in tests).
- Federated learning: ML-KEM-encrypted gradients, secure aggregation inside the enclave, differential-privacy noise with ε-budget accounting. tests/federated_ai_tests.rs: 10 nodes fine-tune a model and no raw sample leaves a node.
- Agent runtime: on-chain agent identity bound to a TEE quote; treasury with spending limits, allow-lists, daily budget, and a human-held kill switch; ZK-verifiable intent for agreements.
- Test: an agent posts a bounty, verifies submitted code by running its tests, pays the developer, and upgrades its own contract through the governance-approved path.

DONE WHEN
VM tests and differential gas tests pass; multi-VM Phase A passes; zkML verify time is measured; reports/05-vm.md is complete.
```

---

## MASTER PROMPT 6: DeFi, Finance, Identity and the Physical World

```
/plan Act as Maya2C's Financial Protocol and Identity Architect. Follow the CLAUDE.md Standing Orders. Build every module below as a native state-machine module (unless noted), behind its own feature flag, with hooks registered in the invariant guard (Master Prompt 8).

1. DEX / AMM
- Native LiquidityPool, ReservePair, LPToken. Constant-product (x·y = k) and concentrated liquidity. Slippage bounds.
- Governance-set fee split: LPs, stakers, PoUW miners / validators, burn.
- Private swaps using the ZK gadgets from Master Prompt 2.
- Frequent batch auctions: one uniform clearing price per block, which removes sandwich attacks at the protocol level.
- Tests: price impact, slippage protection, add/remove liquidity balances, high-volume load simulation.

2. PAYMENT CHANNELS ("Maya Flash") + MACHINE BARTER
- Bidirectional signed channels, HTLCs, penalty transactions for old states, cooperative close, watchtowers.
- Multi-hop pathfinding over a general channel graph (Dijkstra / Yen's k-shortest). A channel graph has cycles, so it cannot be a DAG.
- Streaming micro-payments for machine-to-machine resource barter (compute, bandwidth, storage, power), with bilateral SLA contracts and collateral slashing.
- Tests: 10,000 off-chain transfers settled in one L1 batch; 10,000 simulated IoT devices bartering in real time.

3. CROSS-CHAIN (no multisig bridge operators)
- Bitcoin SPV header tracker with real difficulty-transition rules. Ethereum light client (sync committee + Merkle-Patricia receipt proofs). Generic light-client framework for other L1/L2s.
- Lock/mint and burn/release using light-client proofs; MEV-protected cross-chain atomic-swap pools.
- The ADR states the trust assumptions honestly (Ethereum sync committee; how BTC leaves Maya2C without a custodian).
- Tests: simulated 6-block Bitcoin reorg; native validation of external transactions (tests/crosschain_tests.rs).

4. INSTITUTIONAL FINANCE
- ISO 20022: generate Rust types from the official XSDs for pacs.008, pacs.009, camt.053. Translate incoming messages into (threshold-encrypted) Maya2C transactions; generate valid XML from committed blocks. tests/iso20022_tests.rs.
- RWA: RwaToken, OwnershipCapTable, LegalAttestation, RevenueDistribution. Atomic delivery-versus-payment. Transfer-rule hooks that check identity claims (jurisdiction, accreditation). Test: 10,000 dividend payouts in one block.
- CBDC / permissioned vaults with ZK-KYC. Satellite-oracle updates to commodity collateral values. Test: 50,000 compliant settlements.
- Dark pool: sealed-bid batch matching with MPC/ZK that hides price, size, and counterparties; auditor view keys; ZK solvency proofs. benches/darkpool_bench.rs: 1,000 orders.

5. COMPLIANCE + TAX
- ZK AML range and set-membership proofs; per-jurisdiction tax calculators as pure, tested functions; selective auditor keys. tests/compliance_tests.rs: 500 transactions.
- Add docs/LEGAL_NOTICE.md: this is tooling, not legal advice; lawyers must review jurisdiction rules.

6. MACRO-ECONOMIC ENGINES (simulation first)
- Monetary policy (velocity, liquidity depth, rates), cross-border currency stabilizer, multi-agent liquidity shield, multi-currency collateral debt registry.
- Build these as an agent-based economic simulator first. Shock scenarios: 30% currency devaluation, 50% market crash, liquidity shock. Only parameter sets proven stable in simulation may become governance proposals.
- Real-time ZK proof-of-reserves.
- Docs and UI must say clearly: no module guarantees price stability.

7. IDENTITY
- did:maya2c method: DidDocument, ServiceEndpoint, CryptographicAttestation. Registration, key rotation, and revocation with PQ signatures. W3C VC 2.0 credentials.
- ZK selective disclosure: age ≥ 18, country of residence, accredited status.
- Proof-of-personhood: biometric fuzzy commitments (iris / gait) processed only on the device or in a TEE; the chain stores commitments only. TEE liveness checks against deepfakes.
- EEG/BCI authentication = RESEARCH SIM: EEG feature parser, commitment circuit, proofs expire after 3 seconds, replay and spoof rejection tests.
- Test: 100,000 private verifications with zero biometric data leaving the device.

8. PHYSICAL WORLD / DePIN
- Device attestation: TPM 2.0, secure element, PUF (SIM); enrollment of ESP32 / Cortex-M devices; lightweight PQ-signed telemetry; on-chain tamper flags. End-to-end enrollment → telemetry → tamper test.
- Energy: REAL protocol parsers for Modbus/TCP, IEC 61850, IEEE 1547 (grid is SIM). Pricing from frequency deviation Δf; green-energy certificates minted on verified generation; carbon-offset contracts.
  Tests: 50,000 battery events settle in one epoch; a 500 MW renewable surge is absorbed by scaling PoUW up within 10 ms (sim); 1,000,000 parallel micro-power transfers.
  "Room-temperature superconductor" feeds become a generic high-rate power-telemetry SIM (that material is unconfirmed science).
- Satellite oracle: ingest real Sentinel-2 / Landsat data, compute NDVI / carbon indicators, pay parametric insurance. Use several data sources + median + dispute window. tests/oracle_tests.rs.
- Robot swarms: MAVLink + ROS2 serialization, on-chain task micro-auctions, collision avoidance checked off-chain with on-chain commitments. tests/swarm_tests.rs: 100 agents.

DONE WHEN
Every module has tests, a features.toml entry with its class, and registered invariant hooks; reports/06-finance.md is complete.
```

---

## MASTER PROMPT 7: Networking, Transports and Time

```
/plan Act as Maya2C's Network Architect. Follow the CLAUDE.md Standing Orders. Build crates/p2p, crates/net-guard, crates/timing, hal/transport, and ebpf/.

1. P2P CORE
- Latest libp2p + Tokio: QUIC + TCP, gossipsub topics /maya2c/blocks/1, /maya2c/txs/1, /maya2c/dag/1, Kademlia discovery, request-response sync, peer scoring.
- Mempool (Arc<RwLock<HashMap<TxHash, Transaction>>>) validates gossiped transactions against StateDB.
- Tests: 3 in-memory nodes exchange and validate transactions; then larger clusters in sim/.

2. POST-QUANTUM HANDSHAKES
- PQ Noise handshake using DualKem (ML-KEM-1024 + HQC-256, optional X25519) from Master Prompt 2. Decryption needs BOTH secrets.
- Ephemeral keys per session (forward secrecy) plus a forced re-key every 1,000 blocks.
- Measure handshake latency (goal < 100 ms) and encrypted block propagation under high latency.

3. PEER HEALTH + NETWORK DEFENSE
- Byzantine score per peer: invalid signatures, late blocks, double proposals, malformed frames → sandbox channel → ban. Evidence is sent to slashing (Master Prompt 4).
- Self-healing: honest peers repair diverged state using state proofs.
- eBPF/XDP with aya: drop malformed frames and rate-limit per IP at the driver; AF_XDP zero-copy fast path; flatbuffers parsing. benches/ebpf_bench.rs compares against normal sockets and reports the real gain (goal 5x).
- Terabit / DPDK / WDM backplane: document the design; benchmark what the test machine can really do.
- ML anomaly detector in user space feeds XDP maps. Neuromorphic (Loihi / Akida) SNN backend = SIM.
- DDoS lab test with a synthetic flood: no valid transactions dropped.
- Partition resilience (was "planetary defense grid"): a region that is cut off keeps running as a local sub-DAG and merges back later with bridge proofs intact. Test: multi-region fiber cut + attack, no state lost.
- 40% malicious-node sim: classic BFT tolerates < 33% faulty nodes. Above that, prove the network FAILS SAFE (halts, never forks) and that quarantine restores progress.

4. TRANSPORT HAL (one Link trait, many links)
- Link trait fields: MTU, latency, loss, bandwidth, cost.
  REAL: TCP/QUIC; LoRa serial with AX.25 framing; DTN Bundle Protocol v7 (CCSDS); ETSI GS QKD 014 KM-API client (it is a REST API).
  SIM: satellite RF / Ku-band; FSO laser (weather / occlusion fade > 15 dB → RF fallback; beam-steering model); subsea acoustic (1,500 m/s); OAM multiplexing with phase-front recovery; WDM optical backplane; MEO/GEO supernodes with beamforming around weather and solar events; quantum repeaters (entanglement swapping, purification, fidelity F > 0.95 gate, 5,000 km chain).
  RESEARCH SIM: neutrino signaling. Real experiments reached about 0.1 bit/s, so model that rate, with a strong error-correcting decoder for SNR < −20 dB.
- Compact radio frames under 256 bytes: PQ signatures (about 3.3–4.6 KB) cannot fit, so send hashes and fetch signatures on demand, or aggregate at the gateway. Say this honestly in docs.
- Store-and-forward mesh routing for off-grid nodes. Test: 30% packet loss with full state recovery.
- QKD keys are MIXED INTO the PQ handshake and never replace it. QBER ≥ 11% → drop the QKD path, continue on ML-KEM.
- Quantum memory: entanglement cannot carry information faster than light (no-communication theorem). Build it as entanglement-based QKD key supply only, with a decoherence / fidelity monitor. Test name: entanglement_keys_require_classical_channel.
- Orbital routing matrix: line-of-sight, orbits, Doppler. Earth–Moon–Mars sim with 3–22 minute delays: no forks, no desync.

5. TIME
- Timing service with inputs from NTP, PTP, GNSS (GPS / Galileo), and atomic clocks. Relativistic correction model (special + general relativity) for orbital nodes; pulsar timing as a SIM input.
- Consensus only uses bounded-drift checks, never exact wall time.
- Measure the precision actually achieved. Sub-picosecond global agreement over networks is not physically achievable, so set realistic bounds and document them.

DONE WHEN
P2P and PQ-handshake tests pass; the eBPF program loads in a CI VM (or skips with a documented reason); transport sims pass; reports/07-network.md is complete.
```

---

## MASTER PROMPT 8: Security, Formal Verification and Resilience

```
/plan Act as Maya2C's Principal Security Auditor and Formal Methods Lead. Follow the CLAUDE.md Standing Orders.

1. INVARIANT GUARD + CIRCUIT BREAKERS
- Checked before every commit: total supply, AMM reserve ratios, flash-loan bounds, shard balance sums, fee-split totals.
- On failure, the affected module (WASM VM, AMM, ...) goes read-only for 100 blocks and basic transfers keep working.
- tests/exploit_replays.rs: re-entrancy, integer overflow, flash-loan manipulation. Each must trip the breaker immediately.

2. FORMAL VERIFICATION
- Kani: balance math can never overflow u64; decoders never panic.
- Lean 4: use Aeneas (via Charon) to translate the pure state-transition core from Rust into Lean automatically, then prove supply conservation, no tokens from nothing, and that the fee split sums to 100%. Evaluate Verus for proofs written inside Rust.
- CI: any PR that touches crates/state, crates/fees, or crates/ledger-math must re-run the proofs.
- Differential testing: the optimized execution path vs the Lean / reference model on random inputs.

3. FUZZING
- cargo-fuzz + LibAFL targets: tx decode, header decode, P2P frames, PQ handshake, WASM modules, ZK proof decoding. Structure-aware fuzzing with arbitrary. Corpus stored in the repo; OSS-Fuzz-ready layout.
- Z3 symbolic execution on small critical functions.
- tests/fuzz_harness.rs: 1,000,000 mutated payloads, zero crashes.
- Crash → auto-minimize → draft patch + failing test for human review.
- CI: 1-hour fuzzing pass on every release candidate.
- ZK circuit soundness checks: find under-constrained variables.

4. AI-ASSISTED PATCHING (safe version)
- Pipeline: detect → reproduce → AI proposes a patch (it can run inside a TEE) → Z3 / Lean / Kani + full test suite → pull request.
- It NEVER applies itself to production. Live code changes happen only through the on-chain runtime-upgrade path (Master Prompt 9) with a timelock.
- dlopen hot-reloading of native code is banned in consensus nodes, because it breaks determinism and auditability. Record this in an ADR.
- Consensus-wide rollback safeguards: revert unsafe transitions and keep legitimate transactions.
- Neuro-symbolic code generation: AI-generated WASM/eBPF is accepted only after the SMT checks pass and the AST shows no non-determinism.
- "Self-evolving code" = RESEARCH, sandbox only: evolutionary search over WASM optimizations where every candidate must be proven equivalent. Simulate 10,000 generations under attack.
- tests/self_heal_tests.rs: a zero-day re-entrancy case goes from detection to a verified patch PR. Measure the time.

5. CHAOS + ATTACK SIMULATION (in sim/)
- 33% validator crashes, latency spikes up to 5,000 ms, out-of-order DAG vertices, double-spend races, invalid ML-DSA signatures, tampered PoUW solutions, Sybil floods, 51% hash-rate drop, disk corruption, 50% node dropout.
- Assert: no panics, no invalid state, finality resumes within 3 DAG rounds after a partition, full health within 60 s.
- Agentic SRE: TEE-attested monitors re-route around dead peers and repair state with proofs.
- Run cargo test --test chaos_simulator and write a resilience report.

6. THREAT INTELLIGENCE + ATTESTATION
- On-chain ThreatIndicator, AttackAttestation, AutomatedMitigation, each with evidence proofs. An opt-in firewall worker updates eBPF / iptables maps. Global DDoS sim: quarantine within 2 rounds.
- Firmware guard: TPM 2.0 PCR measured-boot check before start; tamper → key zeroization in under 1 s, with keys evacuated to encrypted backup.
- Location proofs: GNSS spoofing detection; NV-diamond magnetometry is SIM.

7. COMPARATIVE BENCHMARKS
- criterion suite: SHA-256, kHeavyHash, Ed25519, ML-DSA, SLH-DSA, hybrid.
- Energy per finalized transaction, with the measurement method stated (RAPL where available).
- CSV + Markdown tables + plots.

8. SUPPLY CHAIN + FULL WORKSPACE AUDIT
- cargo audit, cargo deny, cargo vet, cargo geiger, cargo udeps, cargo machete, Miri on core crates, loom for concurrency, heaptrack for leaks.
- cargo geiger note: it also counts unsafe code in dependencies we cannot change. Target: zero unsafe in OUR core crates, and every other unsafe justified.
- cargo-mutants on the state and fee crates. The mutation score shows whether tests really catch bugs.
- THREAT_MODEL.md (STRIDE for every component) and SECURITY.md (disclosure policy + bug-bounty plan).
- Master audit report with REAL counts: errors, warnings, failing tests, unsafe blocks, coverage %, mutation score. Never invent zeros.

DONE WHEN
reports/08-security.md is complete with raw outputs; every core invariant is either proven or listed as open.
```

---

## MASTER PROMPT 9: Governance, Wallets, Explorer and Developer Ecosystem

```
/plan Act as Maya2C's Product Engineering Lead for governance, clients, and developer experience. Follow the CLAUDE.md Standing Orders.

1. GOVERNANCE
- Proposals, stake-weighted voting, quorum, 72-hour timelock, automatic parameter changes in StateDB (gas limits, block time, difficulty bounds, fee split, slashing rules, emission curve). Private ZK voting; quadratic voting optional (document the Sybil caveat).
- Runtime upgrades: WASM runtime V1 → V2 at a set height, with storage-migration hooks and no pending transactions dropped. tests/governance_tests.rs.
- Admin-key burn ceremony with PROGRESSIVE decentralization: a security council that can only use an emergency pause, and that expires after N epochs unless voters renew it.
- AI "executive council": TEE-attested advisory agents that can SUBMIT proposals but never vote on their own.
- Test the full cycle: propose → vote → timelock → fee change applied on-chain.

2. RPC + API
- jsonrpsee JSON-RPC 2.0: get_balance, send_raw_transaction, get_block_by_height, get_mining_candidate, plus DAG, shard, proof, and validator queries; WebSocket subscriptions for blocks, transactions, and events.
- Axum REST + GraphQL gateway (DAG state, threshold-encrypted submission). OpenAPI 3 via utoipa.

3. WALLETS
- l1-wallet CLI (clap): generate (encrypted keystore, Argon2id KDF), send --to --amount, balance, sign-offline.
- Tauri 2 + Leptos wallet (apps/wallet-gui), desktop and mobile:
  BIP-39 mnemonic → deterministic ML-DSA + SLH-DSA keypairs (document the derivation; PQ keys have no BIP-32 equivalent). Keys only in the OS keychain (Apple Keychain / Android Keystore / Windows Credential Manager).
  Screens: recovery phrase, QR scanner, pending tx history, manual fee control, shielded transfers, DEX swaps, node health, multisig / threshold management, Ledger hooks.
  Air-gapped signing: signed blobs exported as ANIMATED multi-frame QR codes (PQ signatures are too big for one QR). Raw hex broadcast to public RPC.
  Sci-fi "tactical command center" UI with glow, scan lines, live charts and switchable cyberpunk themes, PLUS a calm accessible mode (WCAG AA contrast, reduced motion).
  Passkey unlock, social-recovery guardians, spending limits, address book with phishing warnings.
- WebDriver e2e: create wallet → submit tx → balance updates. Measure FPS (> 60 with WebGL).

4. EXPLORER, PORTAL, FAUCET, DEV HUB
- Indexer: Axum + sqlx + PostgreSQL, fed by node block commits.
- Leptos SSR explorer: latest blocks stream, tx inspector, account lookup, validators, hash-rate dashboard, DAG view, WebSocket live updates.
- 3D Three.js DAG star-chart ("holographic lattice", obsidian glass, neon cyan/magenta HUD): orbit, zoom, click a vertex to see PQ signatures, state proofs, metrics. Procedural Web Audio on finality (muted by default). 2D fallback for weak devices.
- Faucet (apps/faucet): 1 request per 24 h per IP + address; GitHub OAuth or proof-of-work captcha. Test: 1,000 concurrent requests.
- Telemetry collector + live world map of seed nodes (block propagation, peers, GPU hash rate, DAG depth).
- Portal (apps/portal, Leptos + Tailwind): docs, architecture guides, interactive JSON-RPC playground.
- Dev hub (apps/dev-hub): one-click contract templates, in-browser WASM playground, automatic bytecode security scan before deploy, on-chain usage tracking that pays developers a share of protocol revenue. e2e test: a new developer deploys a dApp in under 5 minutes.
- e2e: web wallet tx → explorer shows it confirmed.

5. SDKs + CLI + DOCS
- uniffi bindings (Python, Kotlin, Swift), Go SDK, TypeScript SDK @maya2c/sdk with wasm-bindgen PQ signing in the browser.
- maya2c-cli: contract compile / deploy, local network simulation, key-vault management.
- Cross-language e2e: each SDK submits a transaction to a local node.
- docs/ on Starlight (Astro): API reference, wallet integration guide, WASM contract tutorial. #![warn(missing_docs)] on public crates; cargo doc --no-deps must be clean.
- .github/workflows/publish_sdk.yml (crates.io, npm, PyPI) runs as DRY-RUN until I approve publishing.

DONE WHEN
Governance cycle, wallet e2e, explorer on local devnet, and SDK e2e tests pass; reports/09-product.md is complete.
```

---

## MASTER PROMPT 10: Infrastructure, Release and Genesis Launch

```
/plan Act as Maya2C's SRE and Release Lead. Follow the CLAUDE.md Standing Orders. IMPORTANT: build everything, but run NOTHING that costs money or touches real servers until I type "APPROVED: <step name>".

1. CONFIG + SERVICES
- config.toml with environment overrides and validation: P2P ports, RPC rate limits, RocksDB cache, consensus mode, pqc level, feature tiers.
- systemd units maya2c-node.service and maya2c-miner.service: ProtectSystem, NoNewPrivileges, MemoryMax, restart on crash.

2. REPRODUCIBLE, SIGNED RELEASES
- Dockerized toolchain; releases use target-cpu=generic (native only for self-builds). Byte-for-byte reproducibility check.
- SHA-256 checksums, Cosign signatures, SLSA provenance, CycloneDX SBOM.
- Dogfooding: attest every Maya2C release with Eric's own attestor tool.
- Targets: Linux x86_64 / aarch64, macOS, Windows.
- .github/workflows/production_build.yml: fmt, clippy, tests, audit, deny, geiger, Lean / Kani, 1-hour fuzz on release candidates.
- Genesis Deployment Bundle: node binaries, CPU/GPU miners + wgpu shaders, Tauri wallets, systemd units, Helm charts, enclave images. Test: release binaries pass a dry-run genesis with no dynamic-linking errors.

3. CONTAINERS + INFRASTRUCTURE-AS-CODE
- Multi-stage Dockerfiles → distroless / static images under 50 MB. docker-compose devnet: Seed-Node, Peer-1, Peer-2 with separate volumes and RPC/P2P ports.
- genesis.json generator (allocations, timestamp, difficulty).
- infra/terraform modules: Hetzner, Vultr, Equinix Metal, AWS, GCP. Regions NA / EU / Asia. 12-node bare-metal plan, 100+ node profile.
- infra/ansible/setup_node.yml: release build, systemd isolation, UFW, Let's Encrypt TLS, eBPF drivers, OS hardening.
- infra/helm/maya2c-cluster: StatefulSets for bootnodes, validators, public RPC; PVCs for RocksDB; ingress load balancing; auto-healing. scripts/k8s_health_check.sh tests rolling upgrades and zero-downtime restarts.
- Print an infracost estimate before any apply. Plan-only by default.

4. OBSERVABILITY
- Prometheus metrics: finality latency, TPS, block propagation, mempool depth, peers, PoW/PoUW difficulty, PQ verify cost, GPU hash rate, DAG depth.
- OpenTelemetry tracing across consensus and cross-shard messages. Grafana dashboards (infra/grafana/dashboards/), alert rules, SLOs with error budgets.
- Health scripts for propagation latency and peer health. tests/telemetry_tests.rs under load.
- CoinGecko / CoinMarketCap-compatible market API.

5. PUBLIC RPC GATEWAY
- Axum + Tonic: JSON-RPC 2.0 + gRPC, caching, rate limits, IP reputation, WebSocket subscriptions, Cloudflare in front. Validators are never exposed publicly.
- scripts/rpc_stress_test.sh: report the measured requests per second (goal 50,000).

6. CREDENTIALS + DEPLOY ORCHESTRATOR
- scripts/configure_environment.sh: hidden input (read -s) for cloud API keys (Hetzner / Vultr / AWS), Cloudflare token + zone + domain, Let's Encrypt email, SSH key path, genesis owner signature. Validate every token against the provider API. Store secrets encrypted (sops + age, or GPG) in .env.production (git-ignored). Generate terraform.tfvars + Ansible inventory. Print a pre-flight checklist (tokens, DNS zones, open ports).
- I run this script myself. Claude must NEVER ask me to paste secrets into chat.
- deploy-production.sh:
  Phase A pre-flight (cargo check, proof logs, secrets present)
  Phase B terraform + Cloudflare DNS
  Phase C Ansible hardening, eBPF, container runtime
  Phase D build, explorer assets, validators + RPC gateways
  Automatic rollback / teardown on any failure. Live TUI progress dashboard. Handles timeouts, blocked ports, and DNS delays with retries. Ends with public IPs, RPC URLs, explorer URL, live TPS, and an ASCII sci-fi launch certificate. --dry-run is the default.

7. GENESIS + LAUNCH
- genesis-ceremony: multi-party collection of validator ML-DSA-87 keys, allocations (including the DAO treasury), sealed genesis DAG root, a transcript signed by every participant and published.
- pre_ignition_audit: runs the full chain (build, tests, Lean/Kani proofs, audit, TEE quote checks where hardware exists, hardware/GPU/eBPF diagnostics) and writes a readiness manifest with raw evidence and a list of KNOWN RISKS.
- LAUNCH.md launch ladder with explicit exit criteria for each gate: localnet → devnet → public testnet → incentivized testnet + bug bounty → external audit → mainnet beta → mainnet.
- Live smoke tests: block height rising, finality time, PQ signatures valid; transfer, contract call, and ZK identity transactions all change state.
- Documented final commands:
  RUSTFLAGS="-C target-cpu=native -C opt-level=3" cargo build --release --bin maya2c-node
  maya2c-node genesis launch --config /etc/maya2c/genesis.toml --consensus dag-bft --pqc-level 5 --enable-pouw --gpu-miner wgpu --verify-all-proofs --exascale-mode
- Autonomy handover: start the agentic monitoring loops (health, failover, maintenance), serve the 3D explorer on the live WebSocket feed, run the admin-key burn under Master Prompt 9's rules. The launch log may print "MAYA2C CORE ACTIVE. INVARIANTS LOCKED. SYSTEM RUNNING AD INFINITUM." but the on-chain governance upgrade path stays open.

DONE WHEN
./deploy-production.sh --dry-run completes end-to-end against a local kind/k3d cluster with 12 simulated nodes; reports/10-launch.md and LAUNCH.md exist.
```