# Maya2C Master Prompts 11–20: From Framework to Production

These ten prompts take the Maya2C framework built by Master Prompts 1–10 and turn it into a product that can be deployed, operated and trusted with real value. Run them in order, one per Claude Code session, from the root of `D:\Maya2C`.

**Order and dependencies**

| Prompt | Phase | Needs |
|---|---|---|
| 11 | Production baseline, launch scope freeze | MP 1–10 |
| 12 | Execution performance (parallel execution, pipelining, storage) | 11 |
| 13 | Post-quantum weight: signatures, bandwidth, data availability | 11 |
| 14 | Horizontal scale: state sync, light clients, sharding, RPC fleet | 12, 13 |
| 15 | Protocol spec, conformance tests, upgrades, client diversity | 11 |
| 16 | Validator and key security in production | 11 |
| 17 | Integration layer: exchanges, custodians, wallets, EVM tooling | 14, 16 |
| 18 | Economic security and launch economics | 12, 13 |
| 19 | Operations at scale and the public testnet program | 14, 16, 17 |
| 20 | Mainnet readiness: audits, go/no-go, genesis, first 90 days | all |

12, 13, 15 and 16 can run in parallel once 11 is done.

**The principle behind this set.** Chains that last are not the ones with the most features at launch. They are the ones whose core never loses funds, never forks unexpectedly, and whose numbers hold up when outsiders measure them. So these prompts do three things: shrink what ships on day one to a core that can be audited, make that core measurably fast under adversarial load, and make every claim reproducible by a third party. Everything from Master Prompts 5–7 marked extended, frontier, SIM or RESEARCH stays in the codebase and activates later through governance, each after its own audit.

---

## MASTER PROMPT 11: Production Baseline, Reality Audit and Launch Scope Freeze

```
/plan Act as Maya2C's Chief Technology Officer and Release Architect. Follow the CLAUDE.md Standing Orders. This phase decides what ships on mainnet day one and sets the rules every later phase must meet. Do NOT add features in this phase.

1. REALITY AUDIT OF MASTER PROMPTS 1–10
- Re-run every DONE WHEN check from Master Prompts 1–10 from a clean clone (cargo clean first). Paste the real output into reports/11-reality-audit.md.
- For every entry in features.toml, re-derive its status from evidence: a feature is "working" only if its listed tests exist and pass now. Downgrade anything that fails. Print a before/after diff of statuses.
- List every TODO, unimplemented!(), todo!(), panic!() on a non-test path, #[ignore]d test, and mocked dependency that sits on a production code path. Save as reports/11-gap-register.md with file:line, owner crate, severity (P0 blocks mainnet / P1 blocks testnet / P2 later).

2. LAUNCH SCOPE FREEZE (write ADR-launch-scope)
- Define the mainnet v1 core. Proposed default (confirm or adjust with evidence from the audit):
  crypto (ML-DSA-65/87, SLH-DSA, ML-KEM, hybrid handshake), types, state, storage, fees, consensus (dag-bft only), mempool, staking/slashing, WASM VM (single Cranelift tier + cache), governance, p2p (TCP/QUIC only), RPC, native token transfers, multisig custody.
- Everything else (multi-VM, zkML, neural gas, agents, DeFi/RWA/CBDC modules, DePIN, exotic transports, PoUW, argonblake-pow, frontier HAL) is DEFERRED: it stays in the repo, keeps its tests, and gets an activation path through governance after its own audit.
- For each deferred module, write one line in the ADR: why it is deferred, what evidence would bring it into scope, and which audit it needs.

3. THE PRODUCTION BUILD PROFILE
- Add a cargo feature `production`. In crates/types (or a tiny crates/build-guard), add compile_error! if `production` is enabled together with ANY sim-*, research-*, frontier-*, test-util, or insecure-* feature.
- `cargo xtask release-check`: builds maya2c-node with --features production, then scans the binary's symbol table and `cargo tree -e features` output to prove no SIM/RESEARCH crate is linked. CI fails otherwise.
- Remove every debug-only RPC method, test key, and devnet faucet path from the production build (feature-gate them; do not delete).
- Consensus mode in production is dag-bft only; other modes fail at config validation with a clear error.

4. PRODUCTION STANDING ORDERS (append to CLAUDE.md under "Production Standing Orders"; they apply from now on)
- The mainnet binary is always built with --features production. No exceptions.
- Every performance number is produced by `cargo xtask bench <name>` and stored with: git commit, hardware spec (CPU model, cores, RAM, disk, NIC), OS, kernel, and the exact command. No number without this record.
- "TPS" always means: signature-verified, executed, state-committed, finalized transactions per second, with the transaction mix stated. No-op or pre-verified transactions are reported separately and labeled.
- Any change to consensus-critical code (state transition, fees, consensus, encoding, crypto verification) needs: a spec update (Master Prompt 15), new conformance vectors, an ADR if behavior changes, and a note that two human reviewers must approve.
- No new dependency in core crates without a `cargo vet` entry and a one-line justification in the PR.
- CI fails on a performance regression above 5% on the tracked benchmarks (Master Prompt 12).
- Secrets never enter the repo, logs, or this chat. Anything that costs money or touches real servers still needs "APPROVED: <step name>".

5. PRODUCTION READINESS SCORECARD
- Create READINESS.md: one row per mainnet-core component with columns: spec written, conformance vectors, unit + property tests, fuzzed (hours), formally verified invariants, benchmarked, externally audited, runbook exists, owner. Every cell is either evidence (link to report/test/commit) or an explicit gap. No checkmarks without evidence.
- `cargo xtask readiness` regenerates the table from features.toml, test results and reports/.

6. SERVICE LEVEL OBJECTIVES (targets, measured later)
- Write docs/SLO.md with initial targets for mainnet core: finality latency p50/p99, block production liveness, RPC availability and latency, state sync time for a new node, maximum tolerated validator downtime before slashing. Mark each as TARGET until Master Prompt 19 measures it.

DONE WHEN
reports/11-reality-audit.md and reports/11-gap-register.md contain real output; ADR-launch-scope exists; `cargo xtask release-check` passes and fails correctly when a sim feature is forced on (show both runs); READINESS.md and docs/SLO.md exist; CLAUDE.md contains the Production Standing Orders; PROGRESS.md lists Master Prompts 11–20.
```

---

## MASTER PROMPT 12: Execution Performance: Parallel Execution, Pipelining and the Storage Engine

```
/plan Act as Maya2C's Principal Performance Engineer. Follow the CLAUDE.md Standing Orders and Production Standing Orders. Scope: mainnet-core only (per ADR-launch-scope). Measure first, optimize second, and never change consensus results for speed.

0. BASELINE BEFORE ANY CHANGE
- Build a workload generator (crates/loadgen, bin maya2c-loadgen): realistic transaction mixes (plain transfers, token transfers, contract calls with shared hot accounts, contract deploys), configurable contention (share of transactions touching the same N hot keys), Zipfian account distribution, seeded and reproducible.
- End-to-end benchmark on one machine and in a 4-validator local cluster: TPS (as defined in the Production Standing Orders), finality latency p50/p99, CPU per stage, disk write amplification, memory. Save to reports/12-baseline.md.
- Profile with perf + cargo flamegraph (and tracy or samply where useful). Identify the top 5 bottlenecks with evidence. Only work on those.

1. PARALLEL EXECUTION (write ADR-parallel-execution)
- Evaluate and choose: Block-STM style optimistic concurrency (multi-version memory, speculative execution, validation, re-execution on conflict) vs declared access lists (transactions declare read/write sets, scheduler builds a conflict graph) vs a hybrid (declared hints + Block-STM as fallback).
- Hard requirement: the parallel result is byte-identical to sequential execution in block order. Commit order is the block order.
- Differential test: 100,000 random blocks with high, medium and low contention; parallel state root == sequential state root every time. Any mismatch prints the seed.
- Gas charged is identical regardless of thread count or re-executions.
- Report speedup curves for 1, 2, 4, 8, 16, 32 threads at 0%, 10%, 50%, 90% contention.

2. PIPELINED ARCHITECTURE
- Separate the node into stages connected by bounded channels: network ingest → signature pre-verification → mempool → consensus ordering → execution → state commitment → persistence → RPC indexing.
- Signature verification happens once, at ingest, in parallel across all cores; the result is cached by tx hash so execution never verifies twice. Invalid signatures never reach the mempool.
- Asynchronous execution (ADR required): consensus orders blocks without waiting for execution; the state root for block N is committed in a later block N + D (choose D, document the effect on light clients and finality semantics). Evaluate the trade-off honestly against synchronous execution.
- Backpressure everywhere: every channel bounded, every queue has a metric, overload sheds lowest-fee traffic first and never stalls consensus.

3. STORAGE ENGINE
- Measure RocksDB as configured today: write amplification, compaction stalls, p99 read latency under load.
- Separate the flat key-value state (fast reads for execution) from the authenticated structure (Jellyfish/sparse Merkle tree for roots and proofs). Update the tree in batches per block, in parallel by subtree.
- Tune: column-family-specific options, block cache sizing, bloom filters, direct I/O where it helps, WAL settings, compaction style. Record every knob change and its measured effect.
- Evaluate (ADR, benchmark-driven): staying on tuned RocksDB vs a custom append-only / io_uring state store (the approach Monad and others took). Build a prototype only if RocksDB is proven to be the top bottleneck after tuning.
- Crash consistency: kill -9 the node at random points during commit 1,000 times in sim/; it must always restart to a valid, consistent state.

4. HOT-PATH HYGIENE
- No allocation in the per-transaction hot loop (verify with a counting allocator in a benchmark). Arena or pooled buffers where needed.
- Zero-copy decode of incoming transactions (rkyv/flatbuffers wire format from Master Prompt 3), canonical encoding only for hashing.
- Memory allocator: benchmark mimalloc and jemalloc vs the system allocator; pick by measurement.
- Profile-guided optimization (cargo-pgo) and BOLT for the release binary; keep it only if the gain is measured and the build stays reproducible.

5. PERFORMANCE REGRESSION CI
- Tracked benchmarks (criterion + end-to-end loadgen runs) on a dedicated, fixed-spec runner (cloud CI is too noisy; document the runner spec). Results stored over time (bencher.dev or a committed CSV history).
- CI fails on > 5% regression in any tracked metric; a PR can only override with a written justification.

DONE WHEN
reports/12-baseline.md and reports/12-performance.md show before/after numbers with full hardware records; the parallel-vs-sequential differential test passes on 100,000 blocks; kill -9 crash test passes 1,000 runs; the regression gate is live in CI; ADRs exist for parallel execution, async execution, and the storage choice.
```

---

## MASTER PROMPT 13: The Post-Quantum Weight Problem: Signatures, Bandwidth and Data Availability

```
/plan Act as Maya2C's Protocol Efficiency Architect. Follow the CLAUDE.md Standing Orders and Production Standing Orders.

CONTEXT
Post-quantum signatures are 20–70x larger than Ed25519 (ML-DSA-65 is about 3.3 KB, ML-DSA-87 about 4.6 KB, SLH-DSA-128s about 7.9 KB; take exact sizes from the crates). At high throughput, signatures and public keys dominate bandwidth, storage and verification CPU. Solving this well is Maya2C's biggest structural advantage over chains that will have to retrofit post-quantum security later. Treat it as a first-class design problem.

1. MEASURE THE WEIGHT
- For the Master Prompt 12 workload: bytes per transaction broken down into signature, public key, payload, overhead. Bandwidth per validator at 1k, 10k, 50k TPS. Storage growth per day. Verification CPU per core. Save as reports/13-weight-baseline.md.

2. KEEP PUBLIC KEYS OFF THE WIRE
- Accounts register their public key once (first transaction or explicit registration). Later transactions carry only a key hash / account ID; validators resolve the key from state.
- Key rotation keeps the account address stable. Test: rotation, then old key rejected, new key accepted.

3. SIGNATURE ECONOMY
- Evaluate FN-DSA (Falcon, FIPS 206) as an additional suite for its much smaller signatures. Check the current standardization status and label it draft if FIPS 206 is not final. Falcon signing uses floating point: signing happens only in wallets, never in consensus; verification is integer-only. Record the decision in the signature ADR.
- Signature pruning after finality: once a block is final, a node may drop transaction signatures from storage and keep a succinct proof that all signatures in the block verified (STARK proof from the Master Prompt 2 proof system). Archive nodes keep everything.
- Validator vote certificates: a quorum certificate with 2f+1 ML-DSA signatures is large. Design and benchmark options: (a) plain list with compact bitmap, (b) STARK-aggregated certificate proving 2f+1 valid signatures, (c) hash-based multi-signature schemes from recent research (RESEARCH label). Pick for mainnet by measured size and proving time; proving must fit inside the block interval on validator-class hardware.

4. VERIFICATION THROUGHPUT
- Parallel verification across all cores at ingest (Master Prompt 12). Measure verifications/second/core per suite.
- GPU verification of ML-DSA batches through the accelerator HAL: result must be re-checked on CPU before it affects consensus, OR the GPU path is used only as a fast pre-filter. Report the real speedup.
- Cache verification results by (tx hash, suite) so re-gossip never triggers re-verification.

5. BLOCK PROPAGATION
- Transactions are gossiped once; blocks/DAG vertices reference batches by digest (Narwhal-style), never resend payloads.
- Compact block relay for any payload that is resent. Measure bytes on the wire per finalized transaction.
- Erasure-coded broadcast of large batches (Reed–Solomon chunks sent to different peers, reconstruct from any k of n) to cut leader upload bandwidth. Benchmark vs plain gossip in sim/ with 100 and 300 validators across 5 simulated regions.

6. DATA AVAILABILITY
- 2D Reed–Solomon erasure coding of block data with data availability sampling, so light nodes can check availability without downloading blocks.
- Commitments: KZG is not post-quantum. Use hash-based (Merkle / FRI-based) commitments for mainnet; if any KZG path exists, label it and keep it out of the production feature.
- Test in sim/: an adversarial proposer withholds 30% of chunks; sampling light clients detect it with the stated probability.

7. WIRE AND STORAGE COMPRESSION
- zstd with trained dictionaries for batches and snapshots (signatures do not compress; say so in the report). Measure the gain on real workload data.

DONE WHEN
reports/13-pq-weight.md shows before/after bytes per finalized transaction, bandwidth per validator at each TPS level, and storage per day, all with hardware records; key registration and rotation tests pass; certificate aggregation choice is justified by measurements in an ADR; DA withholding test passes in sim/.
```

---

## MASTER PROMPT 14: Horizontal Scale: State Sync, Light Clients, Sharding and the RPC Fleet

```
/plan Act as Maya2C's Scalability Architect. Follow the CLAUDE.md Standing Orders and Production Standing Orders.

1. NODE TYPES (write docs/NODE_TYPES.md with measured hardware requirements)
- Validator, full node, archive node, RPC node, light client, and (deferred) compute worker. For each: what it stores, what it verifies, minimum and recommended hardware, bandwidth, and disk growth per month. Fill the numbers from real measurements, not estimates.

2. STATE SYNC (a new node must join fast)
- Snapshot sync: periodic state snapshots at epoch boundaries, split into chunks, each chunk verified against the committed state root with Merkle proofs as it arrives. Parallel download from many peers; a bad chunk bans the peer that sent it.
- Snapshot service: validators and dedicated nodes serve snapshots over p2p; optional HTTPS/object-storage mirror for speed, but the node still verifies every chunk against the on-chain root.
- Warp-style catch-up: after the snapshot, replay only the blocks since the snapshot height.
- Test: a new node joins a sim network with 100 million accounts and reaches head; report the time and bandwidth. Test: a malicious peer serves corrupted chunks and is detected and banned.

3. LIGHT CLIENTS
- Header sync that follows validator-set changes epoch by epoch with finality certificates (aggregated per Master Prompt 13).
- A single succinct proof of "the chain up to epoch E is valid" (recursive STARK over epoch transitions) so a light client can sync from genesis in seconds.
- Light client library in Rust compiled to WASM (browser) and bound through uniffi (mobile). Account balance and transaction inclusion queries with proofs; the client never trusts the RPC server.
- Test: the light client detects a lying RPC server that returns a wrong balance.

4. SHARDING IN PRODUCTION (only if ADR-launch-scope keeps it in v1; otherwise harden it for a later activation)
- Re-audit Master Prompt 4 sharding against the Production Standing Orders. Add: shard assignment of validators with rotation, cross-shard message ordering guarantees, and a proof that a single shard failure cannot corrupt the global supply invariant.
- Load test cross-shard transfers under 50% cross-shard traffic; report latency and throughput separately for same-shard and cross-shard.

5. RPC FLEET
- Stateless RPC nodes behind the public gateway (Master Prompt 10) that read from local full-node state plus a read replica cache. Horizontal scaling: add RPC nodes without touching validators.
- Indexer separation: heavy historical queries go to the Postgres indexer, never to validators or RPC hot paths.
- Per-method cost accounting and rate limits (a getLogs over 1M blocks is not the same cost as get_balance). API keys with tiers for high-volume users.
- Load test: 50,000 req/s mixed read workload across N RPC nodes; report the scaling curve as nodes are added.

6. GEOGRAPHIC DISTRIBUTION
- sim/ scenario with validators spread across realistic inter-region latencies (use published inter-region RTT tables for major cloud regions). Report finality latency p50/p99 per validator-count (50, 100, 200, 400).
- Find the validator count where finality latency or bandwidth breaks the SLO; document it as the current design limit.

DONE WHEN
docs/NODE_TYPES.md has measured requirements; the 100-million-account state sync test passes with reported time; the lying-RPC light client test passes; RPC scaling curve is in reports/14-scale.md; the validator-count limit is measured and documented.
```

---

## MASTER PROMPT 15: Protocol Specification, Conformance and Future-Proof Upgrades

```
/plan Act as Maya2C's Protocol Specification Lead. Follow the CLAUDE.md Standing Orders and Production Standing Orders.

CONTEXT
A chain that only exists as one codebase has one point of failure: one bug in that client can halt or fork the network. Serious chains have a written spec, a conformance test suite, and a path to more than one independent implementation. This prompt builds that path and the machinery for upgrading safely for decades.

1. THE MAYA2C SPECIFICATION (spec/)
- Written, versioned specification of mainnet core: encoding, transaction format and validity rules, state transition, fee market, consensus (DAG construction, commit rule, finality), staking and slashing, networking messages, crypto suites with exact parameters and references to FIPS 203/204/205.
- Executable reference: the pure state-transition and fee logic as a small, slow, readable reference implementation (Python or a minimal Rust crate with no optimizations). Where Master Prompt 8 has Lean/Aeneas models, link them.
- Every spec section has an owner and a version; changes go through a Maya2C Improvement Proposal process (spec/mips/ with a template: motivation, specification, backwards compatibility, security considerations, test vectors).

2. CONFORMANCE TEST SUITE (spec/tests/)
- Language-neutral JSON/YAML vectors: encoding round-trips, valid and invalid transactions with expected errors, state transitions (pre-state, block, post-state root), fee calculations, fork-choice/commit-rule scenarios, crypto KATs.
- Generated from the reference implementation, run against the production node in CI. Any mismatch fails the build.
- Target: every consensus rule has at least one positive and one negative vector. `cargo xtask spec-coverage` reports rules without vectors.

3. CLIENT DIVERSITY PATH
- Build an independent verifier in a second language (Go or TypeScript) that replays blocks from the conformance suite and from a devnet, and checks state roots. It is not a full node; it is a second opinion.
- Document what a second full client would need (docs/SECOND_CLIENT.md) and how to fund it (grants from treasury after launch).

4. PROTOCOL VERSIONING AND ACTIVATION
- Every block carries a protocol version. Features activate at a governance-set height (feature flags in state), never by binary version alone.
- Node refuses to run past an activation height it does not support, with a clear message ("upgrade required before height H"), instead of forking silently.
- Upgrade rehearsal test in sim/: 100 nodes, 70% upgrade before height H, 30% late; the network continues, late nodes halt cleanly, then catch up after upgrading.

5. STATE MIGRATIONS
- Migration framework: versioned migration steps, dry-run mode against a mainnet-size snapshot, measured duration, rollback plan. A migration that would take longer than one block interval must run in the background or across several blocks.

6. CRYPTOGRAPHIC FUTURE-PROOFING
- Extend the crypto-agility engine from Master Prompt 2 into a documented deprecation lifecycle: announce → new suite available → default switch → old suite deprecated → forced migration window → old suite rejected. Each step is a governance action with minimum durations.
- Hash-function agility: the state commitment and block hashing can move to a new hash with a documented migration (re-commitment at an epoch boundary). Rehearse it in sim/.
- Track NIST and IETF post-quantum developments in docs/CRYPTO_WATCH.md (what to re-evaluate and when).

7. API STABILITY
- Semantic versioning for RPC, SDKs and wire protocol. Deprecation policy with minimum support windows. Breaking-change detector in CI for the OpenAPI spec and public Rust APIs (cargo-semver-checks).

DONE WHEN
spec/ covers every mainnet-core rule; conformance vectors run in CI against the node and the independent verifier, both passing; spec-coverage shows zero uncovered consensus rules (or lists them as gaps); upgrade rehearsal and hash-migration rehearsal pass in sim/; reports/15-spec.md is complete.
```

---

## MASTER PROMPT 16: Validator and Key Security in Production

```
/plan Act as Maya2C's Head of Infrastructure Security. Follow the CLAUDE.md Standing Orders and Production Standing Orders. Most real-world validator losses come from key handling and operations, not from broken cryptography. This phase closes that gap.

1. REMOTE SIGNER (bins/maya2c-signer)
- Validator consensus keys never live on the validator host. The node asks a separate signer process over a mutually authenticated PQ channel (Master Prompt 7 handshake).
- Signer backends: encrypted local keystore (dev), HSM via PKCS#11, cloud KMS. Check which HSM and KMS vendors currently support ML-DSA (FIPS 204) and at what certification level; record the findings and the chosen backends in an ADR. Fall back to the encrypted keystore on dedicated hardware where no PQ-capable HSM is available, and say so.
- Threshold signer option for validators that want no single signing host (m-of-n signer nodes), using the custody design from Master Prompt 2.

2. SLASHING PROTECTION
- Slashing-protection database inside the signer: it refuses to sign two different vertices/votes for the same round or any conflicting message, regardless of what the node asks. Persisted with fsync before every signature is released.
- Import/export format (modeled on Ethereum's EIP-3076 interchange format) so operators can move validators between machines safely.
- Tests: node restarted from an old backup asks to re-sign a past round → refused; two nodes running the same key → second one's signatures refused; crash between sign and persist → no double sign possible.

3. SENTRY ARCHITECTURE
- Validators connect only to their own sentry nodes over private links; sentries face the public network. Validator IPs are never gossiped.
- Ansible and Helm templates (Master Prompt 10) updated for validator + 2 sentries by default.

4. DENIAL-OF-SERVICE HARDENING
- PQ handshake flooding: KEM operations and large keys make handshakes more expensive than classical ones. Add a stateless retry/cookie step (like QUIC Retry) and optional client puzzles before any KEM work; per-IP and per-subnet handshake budgets. Benchmark handshakes/second a node can reject and accept.
- Resource accounting per peer: bandwidth, CPU spent on verification, memory held in queues. Peers that cost more than they contribute are throttled then disconnected.
- Mempool: per-sender limits, replacement rules (fee bump percentage), eviction by effective fee; oversized or expensive-to-verify transactions pay proportionally.
- Adversarial tests in sim/: invalid-signature spam at 10x normal load, handshake flood, slow-loris peers, eclipse attempt (attacker controls 70% of a victim's connections). Honest traffic must keep finalizing within SLO.

5. HOST AND SUPPLY-CHAIN HARDENING
- Hardened node image: minimal OS, read-only root, systemd sandboxing (Master Prompt 10), no SSH password auth, automatic security updates policy.
- Reproducible builds verified by at least two independent build machines; release signing keys held offline with a documented ceremony.
- Dependency review gate: cargo vet for all core crates; cargo deny advisories block releases.

6. SECURITY OPERATIONS
- docs/security/INCIDENT_RESPONSE.md: severity levels, who is paged, private disclosure channel, coordinated patch release process for validators before public disclosure, communication templates.
- Security council emergency pause (Master Prompt 9): rehearse it in sim/ end to end, with timing measured.
- Bug bounty scope, rewards table and rules ready to publish (publishing needs "APPROVED: bug-bounty").

DONE WHEN
Remote signer works with the keystore backend and at least one HSM/KMS backend (or the ADR explains why none is available yet); all slashing-protection tests pass; DoS sim scenarios pass with honest finality inside SLO; incident response and emergency pause rehearsals are recorded in reports/16-validator-security.md.
```

---

## MASTER PROMPT 17: The Integration Layer: Exchanges, Custodians, Wallets and Developer Tooling

```
/plan Act as Maya2C's Head of Integrations. Follow the CLAUDE.md Standing Orders and Production Standing Orders. A chain becomes big when exchanges, custodians, wallets and developers can integrate it in days, not months. Build everything they need, documented and tested.

1. EXCHANGE INTEGRATION KIT
- Implement the Mesh API (formerly Rosetta, the Coinbase standard) Data and Construction APIs for Maya2C, and pass the official mesh-cli check:data and check:construction.
- docs/integrations/EXCHANGES.md: finality guarantees in plain numbers (after how many seconds a deposit is irreversible), deposit address strategy, memo/tag handling, withdrawal batching, fee estimation, node requirements, how to handle a chain halt.
- Deposit/withdrawal reference service (Rust): watches finalized blocks, credits deposits exactly once, survives restarts and reorg-free finality edge cases. Test: 100,000 deposits with node restarts in between, zero double credits, zero missed credits.

2. CUSTODY INTEGRATION
- Institutional custody guide: how to hold Maya2C keys in HSMs, how m-of-n on-chain multisig works, how to do offline signing with the air-gapped flow.
- Note in the docs that most existing MPC custody stacks use elliptic-curve threshold schemes that do not apply to ML-DSA; on-chain multisig is the supported path until a threshold lattice scheme is audited.
- Offline signing SDK: build transaction → export unsigned payload → sign on an air-gapped machine → broadcast. Test the full loop.

3. EVM COMPATIBILITY LAYER (only if multi-VM is in launch scope; otherwise prepare it for activation)
- eth_* JSON-RPC namespace so MetaMask, Foundry, Hardhat, ethers and viem work against revm-hosted contracts.
- Security honesty (ADR required): standard EVM wallets sign with secp256k1, which is not post-quantum. EVM-compatible accounts are labeled "classical security" in explorers and wallets. Offer a post-quantum path through account abstraction: smart accounts whose validation logic verifies ML-DSA through a precompile.
- Tests: deploy and use an OpenZeppelin ERC-20 with Foundry against a local node; pass a standard Ethereum JSON-RPC compatibility test set for the supported methods and list the unsupported ones.

4. WALLET CONNECTIVITY
- WalletConnect (Reown) support and a browser-extension wallet provider API for dApps. Chain registration metadata (chain ID, RPC URLs, explorer, token icons) in a machine-readable file for wallet and aggregator listings.
- Hardware wallet flow from Master Prompt 2 tested end to end with the wallet GUI.

5. ORACLES AND DATA
- Native oracle module for mainnet core (price feeds signed by a staked oracle set, median aggregation, staleness checks) OR a documented integration plan for external oracle networks. Choose in an ADR.
- Indexer APIs (GraphQL) stable and versioned; example subgraph-style indexer template for dApp teams.

6. DEVELOPER EXPERIENCE AS A PRODUCT
- `maya2c init` → local devnet in one command → template contract → deploy → verify on local explorer, measured end to end. Target: under 5 minutes on a fresh machine (Windows, macOS, Linux). Record real timings.
- Contract verification service for the explorer (source + compiler version → reproducible Wasm hash).
- Docs site: quickstarts for exchanges, wallets, dApp developers, validators, each tested by a CI job that runs every command in the guide.

DONE WHEN
mesh-cli checks pass; the deposit service passes 100,000 deposits with restarts; the offline signing loop and WalletConnect flow pass e2e; the 5-minute developer path is measured on three OSes; every guide's commands run in CI; reports/17-integrations.md is complete.
```

---

## MASTER PROMPT 18: Economic Security and Launch Economics

```
/plan Act as Maya2C's Protocol Economist and Mechanism Designer. Follow the CLAUDE.md Standing Orders and Production Standing Orders. This phase produces analysis and tooling. It does NOT decide token prices, sales, or legal structure; those need qualified lawyers and advisers, and the reports must say so.

1. ECONOMIC MODEL AS CODE (econ/)
- Agent-based simulator (reuse the Master Prompt 6 framework) for mainnet-core economics: emission schedule, staking yield, fee burn, treasury inflow, validator costs, delegation behavior.
- Parameters load from the same config the chain uses, so the simulator and the chain cannot drift apart.
- Scenarios: low usage for 2 years, sudden 100x usage, 80% price drop, large holder exits staking, fee market spam campaign. Output: staking ratio, real validator income vs costs, supply over time.

2. COST OF ATTACK
- For each attack (halt the chain with > 1/3 stake, finalize a conflicting block with > 2/3, censor transactions, long-range attack on light clients), compute the stake required and the value at risk under several staking ratios and prices.
- Long-range attacks: define weak subjectivity checkpoints and how new nodes obtain them safely; document the period.
- docs/ECONOMIC_SECURITY.md with the tables and their assumptions.

3. VALIDATOR SET HEALTH
- Stake concentration metrics (Nakamoto coefficient, Gini) computed every epoch and exposed in RPC and the explorer.
- Evaluate mechanisms against concentration (stake caps per validator, delegation incentives toward smaller validators). Simulate their effect before proposing any.

4. MEV POLICY
- State the mainnet MEV position in an ADR: which protections are in core (encrypted mempool status from Master Prompt 4, batch auctions), what is left to the market, and what validators are forbidden to do (with slashing evidence where detectable).
- Simulate sandwich and front-running strategies against the core design and report extracted value.

5. FEE MARKET TUNING
- Using Master Prompt 12/13 loadgen data, tune base-fee adjustment speed, block targets, and the multi-dimensional fee weights so that: fees stay low under normal load, spam becomes expensive fast, and the chain never exceeds hardware limits measured in Master Prompt 14.
- Fee estimator RPC method that wallets use; measure its accuracy on simulated traffic.

6. TREASURY AND GRANTS
- Treasury contract rules: spending limits per epoch, multi-stage approval, transparent on-chain reporting. Grants flow for ecosystem teams (including the second-client effort from Master Prompt 15).
- Vesting contracts for allocations with cliff and linear release; tests including early-termination and governance clawback rules if the ADR specifies them.

7. LEGAL READINESS PACKAGE (tooling and questions, not advice)
- docs/legal/QUESTIONS_FOR_COUNSEL.md: token classification questions per target jurisdiction (for example EU MiCA and the US), validator and foundation structure, sanctions screening obligations for front-ends, data protection for identity modules.
- Compliance tooling hooks that front-ends (not the protocol) can use: sanctions list screening, travel-rule message formats for exchanges.
- State clearly: nothing in this repo is legal or financial advice.

DONE WHEN
econ/ simulator runs all scenarios with results in reports/18-economics.md; docs/ECONOMIC_SECURITY.md has cost-of-attack tables; the fee market parameters are justified by measured data; treasury and vesting contracts have passing tests; the legal questions document exists.
```

---

## MASTER PROMPT 19: Operations at Scale and the Public Testnet Program

```
/plan Act as Maya2C's Head of Site Reliability and Testnet Program Lead. Follow the CLAUDE.md Standing Orders and Production Standing Orders. Build everything; run NOTHING that costs money or touches real servers until I type "APPROVED: <step name>". Print an infracost estimate before every approval request.

1. NODE OPERATOR EXPERIENCE
- One-command node install for Linux (package repos: .deb/.rpm) and a container image; config wizard that validates hardware against docs/NODE_TYPES.md.
- Kubernetes operator (crates/k8s-operator, kube-rs): Maya2CNode custom resource handling snapshot restore, rolling upgrades that respect activation heights, validator + sentry topology, and automatic recovery from disk loss via snapshot.
- Automatic snapshot publishing service with verification against on-chain roots.

2. OBSERVABILITY TO SLO
- Every SLO in docs/SLO.md has a metric, a dashboard panel, an alert, and a runbook link. `cargo xtask slo-check` fails if any SLO lacks one of the four.
- Runbooks in docs/runbooks/ for the top 20 failure modes (disk full, fell behind, peer starvation, signer unreachable, clock drift, stuck upgrade, RPC overload, snapshot corrupt, etc.). Each runbook tested in sim/ or on the local k3d cluster.
- Network-wide health dashboard (public): finality, participation rate, validator count, stake distribution, software version distribution.

3. DISASTER RECOVERY
- Scenarios with written procedures and rehearsals: loss of a region, loss of 1/3 of validators, chain halt (liveness failure) and coordinated restart from an agreed height, corrupted state on many nodes after a bug.
- Coordinated restart tooling: validators agree on a restart height and state root, sign it, and restart; rehearse on the local cluster and record timings.

4. LOCAL AND STAGING NETWORKS
- Local: 12-node k3d cluster from Master Prompt 10 extended with sentries, signers, RPC fleet, indexer, explorer.
- Staging: an infrastructure plan for a persistent 50-node devnet across at least 3 regions and 2 providers (plan only until approved). Every release candidate runs there for a soak period before any public release.

5. PUBLIC TESTNET PROGRAM (plans and tooling; launching needs approval)
- Phase 1 public testnet: open node running, faucet, explorer, status page.
- Phase 2 attacknet: invited security researchers get validator slots and a bounty for halting, forking, or censoring; rules of engagement documented.
- Phase 3 incentivized testnet: external validators across many countries and providers; uptime and performance tracked; rewards policy drafted for legal review.
- Load events: scheduled public stress tests with the loadgen at increasing levels; results published with full methodology so anyone can reproduce them.

6. MEASUREMENT AT SCALE
- On the staging network (after approval): measure every SLO, TPS as defined in the Production Standing Orders, finality p50/p99 across real regions, state sync time, RPC capacity. Replace every TARGET in docs/SLO.md with a measured value or a gap.
- Publish a benchmark methodology document so exchanges and researchers can verify the numbers themselves.

DONE WHEN
Kubernetes operator handles install, snapshot restore and rolling upgrade on the local cluster; slo-check passes; the 20 runbooks exist and are rehearsed; coordinated-restart rehearsal is recorded; testnet phase plans and infracost estimates are in reports/19-operations.md, waiting for my approvals.
```

---

## MASTER PROMPT 20: Mainnet Readiness: Audits, Go/No-Go, Genesis and the First 90 Days

```
/plan Act as Maya2C's Launch Director. Follow the CLAUDE.md Standing Orders and Production Standing Orders. You prepare every decision and every artifact; I make the go/no-go calls. Nothing is published, deployed or announced without "APPROVED: <step name>".

1. EXTERNAL AUDIT PREPARATION
- Audit packet per scope: spec sections, architecture docs, threat model (Master Prompt 8), known issues list, build and test instructions, conformance suite, fuzzing corpus and hours, formal proofs and their assumptions.
- Recommended audit scopes (at least two independent firms): (a) cryptography and PQ implementation including side channels, (b) consensus, networking and DoS, (c) state machine, fees, staking and governance, (d) node operations and key management. List what each auditor must receive.
- Findings tracker: every audit finding gets an issue, a fix commit, a regression test, and a re-review status. No mainnet with an open critical or high finding.

2. FREEZE
- Code freeze for mainnet-core with a release branch; only audited fixes merged after freeze, each with a regression test.
- Spec freeze v1.0; conformance suite tagged; genesis parameters frozen in a signed file.

3. GO/NO-GO CHECKLIST (LAUNCH.md, extends Master Prompt 10)
- Every gate has objective, evidence-linked exit criteria, for example:
  zero open critical/high audit findings; attacknet ran at least N weeks without an unresolved halt or fork; incentivized testnet met finality and uptime SLOs for N consecutive weeks; external validators ≥ target count across ≥ target providers and countries with no provider above a set stake share; state sync, restore and coordinated restart rehearsed in the last 30 days; incident response on-call rota staffed; legal sign-off received (I confirm this manually).
- `cargo xtask go-no-go` collects the evidence, prints each gate as PASS / FAIL / NEEDS HUMAN, and never prints PASS without an evidence link.

4. GENESIS
- Genesis ceremony (Master Prompt 10) rehearsed at least twice on the staging network with the real participants' process, then run for real only after approval.
- Genesis validators use remote signers with slashing protection from their first block.
- Launch sequence with named roles, timings, communication channels, and abort criteria (for example: finality not reached within X minutes → coordinated restart procedure).

5. FIRST 90 DAYS
- War room plan: 24/7 on-call for the first 2 weeks, daily health reports, scheduled validator calls.
- Conservative launch parameters (lower gas limits, deferred modules off) with a governance schedule to raise limits as measured capacity allows.
- Hotfix process: private patch to validators, coordinated upgrade at an activation height, public disclosure after the network is safe.
- Metrics review at day 7, 30, 90 against SLOs and economic model predictions; report deviations.

6. POST-LAUNCH ROADMAP (docs/ROADMAP.md)
- Activation order for deferred modules (from ADR-launch-scope), each with its audit requirement and the evidence needed.
- Second client funding, cryptographic watch items, scaling milestones tied to measured limits from Master Prompt 14.

7. FINAL REPORT
- reports/20-mainnet-readiness.md: the go/no-go table with evidence, every KNOWN RISK in plain language with its mitigation and owner, and a clear statement of what is not yet proven.

DONE WHEN
Audit packets exist for every scope; the findings tracker is in place; `cargo xtask go-no-go` runs and reports honestly; genesis rehearsal procedure and launch sequence are written; ROADMAP.md and reports/20-mainnet-readiness.md exist. The actual launch happens only after my explicit approval of each gate.
```
