# Next-evolution gap table

Reconciles every feature and claim in [next-evolution.md](next-evolution.md) (69 rows) against the tree.
Status is from `features.toml` (`class / status`) and `docs/architecture-vision.md` where both exist; the document wins on disagreement (`features.toml` header). Maya OS is a separate plan: [maya-os.md](maya-os.md).

**Gap size.** S: config, docs or wiring of code that exists. M: promote or extend one crate, with tests. L: new subsystem or research program; needs an ADR and possibly a new invariant.

**Band.** P2 verify what is claimed working. P4 adoption: SDKs, tooling, wallet and explorer UX. P5 differentiators: privacy, interop, identity. P6 deferred and frontier. Per CLAUDE.md, nothing in P6 starts without Eric's instruction.

**Eric decision?** Yes if the item changes economics, consensus, a trusted party, a new cryptographic scheme, money, legal exposure or the roadmap order. No if it is engineering inside an agreed plan.

Standing Orders 2 and 10 apply to every row: the brief's numbers and superlatives are targets, not claims.

## Mandatory features

| # | Brief asks | What exists | Gap | Invariant collisions | Band | Eric decision? |
|---|---|---|---|---|---|---|
| M1 | Hybrid BlockDAG + blockchain | `crates/dag-bft` (DAG-BFT ordering, REAL verified); `crates/blockgraph` (multi-shard DAG, RESEARCH working, nothing in consensus references it); Ethash-style DAG PoW (REAL verified) | M | 24 (block id and roots cover the body), 26 (blocks and state commit in one batch) | P6 | Yes: a second ordering layer is a consensus change |
| M2 | Deterministic finality | `dag-bft` REAL verified (`docs/architecture-vision.md` §2); attested checkpoints, ADR-038 | S | 24 | P2 | No |
| M3 | Near-instant confirmation | No measurement. Finalized-TPS and finality latency not measured (ADR-059 is the method; Standing Order 2) | M | None | P2 | No |
| M4 | Horizontal scalability | `crates/blockgraph` shard map (RESEARCH; "not consensus"); no measured scaling | L | 25 (every record under the state root), 26 | P6 | Yes: sharding splits state across lanes |
| M5 | Dynamic sharding | `blockgraph/shard_manager` (RESEARCH working; bisect and merge, not in consensus); `shard_of` fixed for fee features | L | 24, 25, 27 (pruning per shard) | P6 | Yes |
| M6 | Stateless validation | `crates/stateless-core`, `crates/node/src/state/stateless.rs`, `crates/light-client` (RESEARCH working; `STATELESS_ACTIVATION_HEIGHT = u64::MAX`; transfer-only blocks decidable) | M | 24, 25 (sealed, oracle and governance records block it) | P6 | Yes: promotion is a written decision (CLAUDE.md, RESEARCH) |
| M7 | Parallel execution engine | `crates/parallel-exec` (ADR-017; RESEARCH working; "not in the node") | M | 24 (the only apply path), 26 | P6 | Yes |
| M8 | WASM virtual machine | `crates/vm`, Cranelift JIT (REAL verified); `vm-execution-tiers` Cranelift and Pulley with identical gas (RESEARCH working) | S | 21 (a host function that does native work charges fuel first) | P2 | No |
| M9 | Rust-native development environment | `xtask`, `bins/maya2c-cli` (REAL working: dev chain, time-travel debugger, replay); `sdks/` | S | None | P4 | No |

## Cryptography

| # | Brief asks | What exists | Gap | Invariant collisions | Band | Eric decision? |
|---|---|---|---|---|---|---|
| C1 | Default privacy | `crates/zk-stark` shielded joinsplit (REAL verified). Mainnet v1 ships without shielded transfers (`mainnet-v1-scope.md`); the pool is opt-in, not default | M | 25, 31 (suite-tagged transactions), 32 | P5 | Yes: default-on privacy changes what mainnet ships |
| C2 | Zero-knowledge proofs | Transparent STARKs over BabyBear, Plonky3 (ADR-008, REAL working). halo2/KZG retired (`docs/architecture-vision.md` §1) | S | None: the STARK route is post-quantum (ADR-008) | P5 | No |
| C3 | Homomorphic encryption | None. `crates/permissioned-finance/src/mpc_spdz.rs` names HE only as an alternative | L | 29, 30 (a new scheme is a suite: ACVP vectors, closed registry) | P6 | Yes: new cryptography |
| C4 | Stealth addresses | None (repo-wide search, 0 hits) | M | 29, 30, 31 (a new address form changes transaction verification) | P5 | Yes |
| C5 | Ring signatures | None (0 hits) | L | 29, 30, 31 | P6 | Yes: new cryptography; peer review first |
| C6 | Selective disclosure | `selective-disclosure-credentials` (`crates/zk-stark/src/credential.rs`, RESEARCH working); `pq-viewing-keys` (`crates/privacy`, RESEARCH working) | S | 31 | P5 | No: promotion is a written decision, Eric's to make when ready |
| C7 | Compliance proofs | `crates/zk-stark/src/sanctions.rs` (RESEARCH working); `crates/iso20022` (refuses mainnet, RESEARCH) | M | 11 (the sanctions list publisher and the ISO gateway are trusted parties) | P5 | Yes: regulatory and legal exposure |
| C8 | Post-quantum cryptography | `crates/crypto-pq`: ML-DSA, ML-KEM, SLH-DSA, FIPS 204, 205 (REAL verified); HQC draft (RESEARCH) | S | 2, 4, 29, 30 | P2 | No, except the ML-DSA-87 choice (decided 2026-10-28, memory) |
| C9 | Quantum-safe wallet recovery | `vault-accounts` (`crates/node/src/state/vault.rs`, REAL verified); guardian recovery, ADR-056; `crates/smart-account` (RESEARCH) | M | 32 (vault limits and delay), 31 | P4 | No |

## AI-native infrastructure

| # | Brief asks | What exists | Gap | Invariant collisions | Band | Eric decision? |
|---|---|---|---|---|---|---|
| A1 | Decentralized AI agent framework | `bins/maya2c-mcp` (REAL working, an MCP server for AI assistants, not an agent runtime); `crates/confidential-ai` (off-chain, RESEARCH) | L | 13 (no governance key's value is a program), 21 | P6 | Yes |
| A2 | On-chain autonomous agents | `crates/smart-account` scoped session keys (RESEARCH); nothing executes an agent | L | 12, 13, 24 | P6 | Yes |
| A3 | AI marketplace | None | L | 11 (escrow or arbiter as trusted party), 25 | P6 | Yes |
| A4 | AI execution proofs | zkML removed (ADR-008; invariants 20 to 23 retired, 21 still binds `host_verify_zkml_proof`). Transparent STARK toolchain exists, no inference circuit | L | 20, 21, 22, 23 | P6 | Yes: ADR-069 item 9 is the planned path |
| A5 | AI-to-AI payment protocols | Session keys and hash-lock HTLC atomic swaps (REAL verified, `crates/htlc-lattice`); no agent payment protocol | M | 31, 32 | P6 | Yes |
| A6 | AI contract generation | None. `crates/contract-safety` (RESEARCH working) provides rules the output would be checked against | L | 13, 21 | P6 | Yes: generated code reaches chain state |
| A7 | AI validator assistance | None. ADR-069 item 8 (on-chain AI judge) is Proposed | L | 11 (an advisor that moves committee choices is a trusted party), 12, 24 | P6 | Yes |
| A8 | Autonomous treasury management | `crates/treasury` (RESEARCH working): spending limits, staged approval; genesis treasury has no spend path | L | 12, 13, 17 (governed values read from state), 32 | P6 | Yes: money |

## Smart contracts

| # | Brief asks | What exists | Gap | Invariant collisions | Band | Eric decision? |
|---|---|---|---|---|---|---|
| S1 | Private smart contracts | `private-contract-state` (`crates/zk-stark/src/private_state.rs`, `crates/vm/src/private.rs`, RESEARCH working) | M | 21, 25 | P5 | No: promotion decision |
| S2 | Upgradeable privacy-preserving state | Partial: S1; `hash-migration-rehearsal` (RESEARCH working) | L | 13 (an upgrade must not fetch and run native code from state), 24, 25 | P6 | Yes |
| S3 | Formal verification | Kani REAL verified on six crates (`ledger-math`, `dex`, `governance`, `fee-market`, `threat-intel`, `amount`). Lean 4 PLANNED | S (Kani), L (Lean) | 1, 6 (dependency freedom for Kani) | P2 (Kani), P6 (Lean) | Yes for Lean |
| S4 | Natural-language programming | None | L | 13, 21 | P6 | Yes: safety-critical |
| S5 | Self-auditing contracts | `contract-safety` (RESEARCH working); `exploit-replay-suite`, `state-invariant-guard` (REAL verified) | M | 28 (guard refuses, never halts a healthy path) | P5 | No |
| S6 | AI-assisted debugging | `bins/maya2c-cli` time-travel debugger, DAP for VS Code (REAL working); `bins/maya2c-mcp` (REAL working) | S | None | P4 | No |
| S7 | Built-in security verification | `state-invariant-guard`, `exploit-replay-suite`, fuzzing, `supply-chain-gates` (all REAL verified) | M | 28 | P2 | No |

## Consensus

| # | Brief asks | What exists | Gap | Invariant collisions | Band | Eric decision? |
|---|---|---|---|---|---|---|
| K1 | Byzantine fault tolerance | `crates/dag-bft` (Narwhal + Bullshark, REAL verified); ADR-052 follower advance, ADR-070 committee floor | S | 12 (the governance quorum floor is compiled in), 24 | P2 | No |
| K2 | Adaptive validator selection | `crates/staking` committee selection and slashing (ADR-028, REAL verified) | M | 12, 17 | P5 | Yes: changes who validates |
| K3 | Geographic decentralization incentives | `staking/concentration.rs` measures concentration; ADR-069 item 3 (planned reward split) | M | 12, 13 (the reward rule is code, not a governed program), 14, 15 (telemetry is unverified) | P5 | Yes: economics |
| K4 | Self-healing consensus | `protocol-upgrade-halt` (REAL working, halt before fork); ADR-052, ADR-041 follower recovery. No automatic healing | L | 12, 24, 28 | P6 | Yes |
| K5 | Anti-MEV protection | `crates/mev` threshold-encrypted mempool (REAL verified, ADR-025); `crates/node/src/sealed` | S | 11 (the sealed-mempool committee is an optional trusted set) | P4 | No |
| K6 | Fair transaction ordering | Sealed mempool hides content; ordering is the proposer's choice. No fair-ordering rule | M | 24 | P5 | Yes: consensus change, two human reviewers (CLAUDE.md) |
| K7 | Sybil-resistant validator selection | Stake-based (REAL). Personhood is RESEARCH: no liveness, no uniqueness (`crates/personhood`) | L | 11 (liveness needs TEE attestation) | P5 | Yes: the Phase 5 prerequisite for governance |

## Interoperability

| # | Brief asks | What exists | Gap | Invariant collisions | Band | Eric decision? |
|---|---|---|---|---|---|---|
| I1 | Universal bridge framework | `crates/interop` (headers, route caps, intents; RESEARCH working); `crates/btc-bridge` (RESEARCH working); `docs/prior-art/trustless-interop.md` | L | 11 (each bridge is a trusted party), 25, 27 | P5 | Yes |
| I2 | Native cross-chain messaging | None native; `eth-beacon-light-client` (RESEARCH verified) | L | 11, 24 | P6 | Yes |
| I3 | Shared liquidity layer | `crates/dex` (constant product and order book, REAL verified); `crates/l2-flash` (REAL verified) | L | 7, 8, 9 (DEX and oracle write paths), 25 | P5 | Yes |
| I4 | Multi-chain asset management | `crates/btc-spv`, `crates/btc-bridge` (RESEARCH); `crates/multivm` (revm and sbpf, RESEARCH) | L | 11, 25 | P6 | Yes |
| I5 | Chain abstraction | Intents only (`crates/interop`); no abstraction layer | L | 31 | P6 | Yes |

## Identity layer

| # | Brief asks | What exists | Gap | Invariant collisions | Band | Eric decision? |
|---|---|---|---|---|---|---|
| ID1 | Decentralized identity | `crates/identity` (RESEARCH working), DID key rotation (`docs/identity.md`) | M | 25, 29 | P5 | No |
| ID2 | Verifiable credentials | `crates/zk-stark/src/credential.rs` (RESEARCH working); issuer anchors on chain | M | 11 (issuer is trusted), 31 | P5 | No |
| ID3 | Reputation systems | `crates/threat-intel` score (RESEARCH working) only; no reputation for people | L | 12, 14, 15 | P6 | Yes: Sybil gaming makes it meaningless without ID4 |
| ID4 | Human verification layers | `crates/personhood` (RESEARCH working): no liveness, no uniqueness; `hal/bci-sim` (SIM) | L | 11 (liveness needs a TEE) | P5 | Yes: biometrics, legal and ethical |
| ID5 | Privacy-preserving KYC | `crates/permissioned-finance` CBDC vault with STARK KYC admission (RESEARCH working); sanctions proofs | M | 11 (KYC issuer), 32 (per-tier limits) | P5 | Yes: legal |

## Economic model

| # | Brief asks | What exists | Gap | Invariant collisions | Band | Eric decision? |
|---|---|---|---|---|---|---|
| E1 | Sustainable validator economics | `crates/staking` (REAL verified); `econ` simulator (SIM; prices are inputs, never forecasts); ADR-044 block rewards | M | 12, 17 | P2 | Yes: economics |
| E2 | Adaptive fee markets | EIP-1559 base fee (REAL verified, `crates/fee-market`); `neural-base-fee-gain` (RESEARCH working) | S | 13, 20 (retired text: floats in consensus rules are a rounding risk). A neural base fee would need floats | P2 | No |
| E3 | Decentralized treasury | `crates/treasury` (RESEARCH working); genesis treasury has no spend path | M | 12, 13, 17, 32 | P5 | Yes: money |
| E4 | Community governance | `crates/governance` lifecycle and bounds (REAL verified); representative voting is Phase 6 and needs PoP first | L | 12, 13, 17 | P5 | Yes |
| E5 | Public goods funding | None | M | 12, 13 | P6 | Yes: money |
| E6 | Deflationary and growth-balanced tokenomics | ADR-044 block rewards, ADR-046 256-bit supply; 21M cap kept (memory, 2026-10-28) | S | 12, 17 | P5 | Yes: economics. Memory records changes; confirm before any ADR |

## Infrastructure

| # | Brief asks | What exists | Gap | Invariant collisions | Band | Eric decision? |
|---|---|---|---|---|---|---|
| F1 | Mobile validators | `crates/signer` and sealed keys (REAL working) on desktop only; `apps/wallet-gui` mobile targets scaffolded | L | 11 (a phone validator needs TEE), 5 (the signing path) | P6 | Yes: store rules forbid phone mining; see `maya-os.md` |
| F2 | Browser validators | `sdks/sdk-wasm` (client SDK only) | L | 11, 27 (a browser cannot keep archives) | P6 | Yes |
| F3 | IoT nodes | `hal/iot-anchor` (RESEARCH working, `IOT_ACTIVATION_HEIGHT = u64::MAX`); `hal/iot-firmware` (`no_std`, QEMU) | M | 4 (ML-DSA-65 only on devices), 11 (vendor certificates not verified) | P6 | Yes |
| F4 | Edge computing support | `hal/ebpf-net` (RESEARCH working, Linux); `hal/radio-transport` (RESEARCH working) | L | 25 | P6 | Yes |
| F5 | Decentralized storage | `crates/da` (2D Reed-Solomon and sampling, RESEARCH working); `crates/archive` (historical pruning and archive bootstrap, REAL verified); `crates/state-sync` (RESEARCH working) | M | 26, 27 (no body deleted before a verified copy), 25 | P5 | Yes: storage incentives are economics |
| F6 | Distributed AI computation | `crates/lattice-pow` (RESEARCH working); `hal/cuda-miner`, `hal/wgpu-miner` (REAL verified, mining only); `crates/confidential-ai` (off-chain) | L | 11, 13, 21 | P6 | Yes |

## Developer experience

| # | Brief asks | What exists | Gap | Invariant collisions | Band | Eric decision? |
|---|---|---|---|---|---|---|
| D1 | SDKs in Rust, Go, TypeScript, Python and C++ | Go and Python SDKs, JS and WASM SDKs (REAL verified, `sdks/`). `sdks/sdk-ffi` generates UniFFI bindings; no C++ target exists | S (C++ wrapper and example) | None | P4 | No |
| D2 | AI coding assistant | `bins/maya2c-mcp` (REAL working) | M | None | P4 | No |
| D3 | One-click deployment | `one-line-node-join` (REAL working); `k8s-operator` (REAL working); ADR-060 trusted-block bootstrap | S | None | P4 | No |
| D4 | Simulation environment | `deterministic-simulation-harness` (SIM verified); `network-simulation-harness` (REAL verified); `econ` | S | None | P4 | No |
| D5 | Automated contract testing | `exploit-replay-suite`, fuzzing (REAL verified); deterministic replay in `maya2c-cli` | M | 28 | P4 | No |
| D6 | Formal verification tooling | Kani (REAL verified, arithmetic crates only); Lean 4 (PLANNED) | L | 1, 6 | P4 (Kani), P6 (Lean) | Yes for Lean |

## Goal

| # | Brief asks | What exists | Gap | Invariant collisions | Band | Eric decision? |
|---|---|---|---|---|---|---|
| G1 | A complete L1 architecture, consensus design, cryptographic model, tokenomics, developer ecosystem and governance framework that "could realistically serve billions" and "surpass every existing blockchain" | `docs/architecture-vision.md`, ADRs, `docs/invariants.md`, `features.toml`. Scale and superiority claims have no measurement and no prior-art search (Standing Order 10) | L | 12, 17 (tokenomics), and every row above | P5 | Yes: tokenomics and governance |

Row count: 9 + 9 + 8 + 7 + 7 + 5 + 5 + 6 + 6 + 6 + 1 = **69**.

## Top 10 post-mainnet items

Ranked by how many rows each unblocks, then by user impact. Each names its band and whether Eric decides.

1. **Measure finality and finalized TPS (M2, M3, S for K1).** P2. Measured, not declared, per ADR-059 and Standing Order 2. Blocks any "near-instant" claim. No Eric decision.
2. **Economics beyond launch (E1, E6, K3, E3).** The launch economics are decided: GPU-mined block rewards, no premine, a forever tail reward and a 21M cap (ADR-044 and ADR-068, DECISIONS 2026-10-28). Still open for Eric: the treasury share, geographic incentives (K3), and any later change to the issuance schedule. P5.
3. **Wire the genesis treasury spend path under invariants 12, 13 and 17 (E3, E5, A8).** P5. Medium, and it unlocks public goods funding. Yes: money.
4. **Promote private contract state and the shielded default (C1, S1, S5).** P5. Medium. Yes: default privacy changes what mainnet ships.
5. **Decide the human-verification model and its TEE question (ID4, K7, E4, ID3).** P5. The governance prerequisite. Yes: resolves invariant 11 for personhood.
6. **Add stealth addresses over ML-KEM (C4).** P5. Medium, with a new suite under invariants 29 to 31. Yes.
7. **Promote decentralized storage with sampling (F5, M6 partly).** P5. Medium. Supports light clients and the "billions" goal without a new cryptosystem.
8. **Scam Shield: duress keys and safety-window payments (ADR-069 items 1 and 2).** P4. Medium. User impact ranks it highest; not in the brief. Yes: consensus account rule.
9. **Add a C++ SDK wrapper over `sdk-ffi` (D1).** P4. Small. Closes one language in the brief with no consensus change.
10. **Extend Kani to the remaining consensus crates; decide on Lean (S3, D6).** P2 for Kani, P6 for Lean. Small for Kani; Lean is a new frontier module, so Eric decides.

Not on this list, on purpose: homomorphic encryption, ring signatures, mobile and browser validators, natural-language programming and AI validator assistance. Each is L and collides with invariants 11 or 13, or needs new cryptography. They go to P6 until Eric decides.

## Discrepancies found

- `entropy-hal`: `features.toml` REAL verified; `docs/architecture-vision.md` §1 RESEARCH.
- Invariants 20 to 23 are retired (zkML, ONNX). CLAUDE.md Standing Order 4 and Execution Directive 2 cite invariant 20 for "no floats in consensus". The number does not carry that rule.
- Two files numbered ADR-071: resolved 2026-11-01, the base-fee ADR is now ADR-076.
- `features.toml` lists `maya2c-hub` as class REAL with status planned.
- `docs/prior-art/` has dated notes for some rows (trustless interop, native smart accounts, local fee markets, quantum harbor, privacy viewing keys). Rows without one (C3, C5, A1 to A8, I5, K6) cannot make a "first" or "only" claim (Standing Order 10).
