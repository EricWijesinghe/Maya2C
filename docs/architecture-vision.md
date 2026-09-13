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
| HQC (Hamming Quasi-Cyclic) | **SHIPPED** | `crypto-pq/src/hqc.rs` | Second KEM, available as a fallback. Conditions for enabling it are in [pq-transport.md](pq-transport.md) |
| Zero-knowledge proofs (arkworks) | **SHIPPED** | `zk-privacy` | Groth16 shielded joinsplits |
| Zero-knowledge proofs (halo2/KZG) | **RESEARCH** | `zkml` | Verifier only, dark — see §2 |
| MPC-TSS threshold custody (*m*-of-*n*) | **SHIPPED** | `custody-mpc` | Dealerless Pedersen VSS, ML-KEM-sealed shares. Protects the 32-byte chain key, not either signature — invariants 18, 19 and [custody-mpc.md](custody-mpc.md) |
| Verifiable random function (RFC 9381) | **SHIPPED** | `vrf` | Suite octet `0x03`; the RFC's own vectors are pinned — invariant 10 |
| Threshold-encrypted mempool | **SHIPPED** | `mev`, `src/sealed/`, `src/state/sealed_exec.rs` | A miner orders transactions it cannot read. `settle_sealed` runs every block from `stage_block`; the crate itself stays chain-free, curve arithmetic and an AEAD. Optional at genesis like the oracle — a chain with no committee accepts no envelope. **Not post-quantum**: the KEM half is Ristretto ElGamal, because threshold ElGamal has no ML-KEM analogue |
| Self-sovereign identity records | **RESEARCH** | `identity`, `src/state/identity.rs` | `did:maya2c:<address>` — the 32-byte address, which is blake3 over *both* public keys, so a DID inherits the hybrid binding `Transaction::sender()` argues for. A 1,984-byte hybrid key would be a 2,712-character DID |
| DID key rotation and revocation | **RESEARCH** | `src/state/identity_exec.rs` | Authorised by the *old* key. No new proof system: ML-DSA-65 is already a lattice signature and every transaction carries one |
| Selective disclosure credentials | **RESEARCH** | `zk-privacy/src/credential.rs` | Issuer anchors a Poseidon root on chain, signed with the hybrid pair and verified **natively** by consensus; the holder proves membership, a predicate and an unset revocation bit in-circuit. Verifying ML-DSA inside a circuit would be 10^7–10^8 constraints, so the issuer's signature never enters one. **The holder's proof is Groth16 and is not post-quantum** |
| Lattice HTLC-L atomic swaps | **RESEARCH** | `htlc-lattice`, `htlc-watcher`, `src/state/htlc_exec.rs` | `HTLC_L_ACTIVATION_HEIGHT = u64::MAX`. The lock is a Module-LWE instance `t = A·s + e` under ML-DSA-65's parameters; a claim reveals the short `(s, e)` before `expiry_height`. **Maya2C↔Maya2C only**: an atomic swap needs both chains to check the same predicate, and Bitcoin cannot check this one. Claims are never gated by the circuit breaker, because a halted claim beside a live refund is theft — [htlc-lattice.md](htlc-lattice.md) |
| Physical QKD, KM-API interface | **PLANNED** | — | ETSI GS QKD 014-style key-management interface to external QKD hardware. No code |

## 2. Consensus and execution

| Component | Status | Where | Notes |
|---|---|---|---|
| Hybrid-signature transaction validation | **SHIPPED** | `src/` | Both halves verify or the transaction fails |
| Ethash-style DAG proof of work | **SHIPPED** | `src/crypto/dag/` | With an `activation_height` registry — [dag-pow.md](dag-pow.md) |
| Committed transaction root + state root | **SHIPPED** | `src/state/`, `src/chain.rs` | `BlockHeader::tx_root`, checked before anything is stored — invariant 24 |
| Undo journal and reorg safety | **SHIPPED** | `src/state/` | Every consensus record's prior value is journalled — invariants 8, 25, 26 |
| WASM runtime via Cranelift JIT | **SHIPPED** | `vm` | Gas is wasmtime fuel; native host work is charged first — invariant 21. Compiled modules are cached, keyed by bytecode *and* a digest of the engine configuration, so a config change invalidates every entry rather than serving code built under the old compiler. Measured 14x on a working contract, 23x on a trivial one — [docs/vm-module-cache.md](docs/vm-module-cache.md) |
| Historical pruning + archive bootstrap | **SHIPPED** | `archive`, `src/state/blocks.rs` | No body deleted before a verified copy exists — invariant 27, [pruning.md](pruning.md) |
| Constant-product DEX + order book | **SHIPPED** | `dex` | Dependency-free for Kani. A losing trade is a no-op — invariants 6, 7 |
| Governance lifecycle and bounds | **SHIPPED** | `governance` | Governance cannot make governance unsafe — invariants 12, 13, 17 |
| EIP-1559 base fee over bytes | **RESEARCH** | `fee-market` | `FeeConfig::DISABLED`, activation `u64::MAX`. Called by nothing in `src/` |
| Neural base-fee gain | **RESEARCH** | `fee-market/src/model/`, `src/neural_gas/`, `neural-gas-trainer` | A 6-16-1 integer network, i16 weights compiled in, scales EIP-1559's step by a one-sided gain — a rise ×1 to ×2, a fall ×0 to ×1 — so no feature a block producer writes can price a block below EIP-1559. Native inference, **no zkML**: every validator can re-run 112 multiplies, and a proof would cost ~10⁴× that. Trained on a synthetic demand simulator, because no chain history exists — [neural-gas.md](neural-gas.md) |
| Multi-shard asynchronous DAG (Narwhal/Tusk) | **RESEARCH** | `blockgraph` | Batch references and deterministic shard scheduling. Nothing in consensus references a batch yet — [blockgraph.md](blockgraph.md) |
| Lattice Proof-of-Useful-Work (SVP solver) | **RESEARCH** | `lattice-pow` | Verification only, dependency-free. The open question is in [lattice-pow.md](lattice-pow.md) |
| On-chain zkML ONNX inference | **RESEARCH** | `zkml`, `zkml-prover` | `ZKML_ACTIVATION_HEIGHT = u64::MAX`. SRS is from a public seed, so proofs are forgeable — invariants 20, 22, 23 and [zkml.md](zkml.md) |
| Universal EVM / SVM / Move transpilation | **PLANNED** | — | Foreign bytecode lowered to the WASM runtime. No code. The hard part is not the lowering — it is that every source VM has its own gas semantics, and a transpiled contract must be priced by the fuel meter without inheriting them |

## 3. Kernel, networking and transport

The node speaks libp2p over a post-quantum Noise handshake today. Everything
below that line is intent.

| Component | Status | Where | Notes |
|---|---|---|---|
| libp2p transport, PQ Noise handshake | **SHIPPED** | `src/network/` | ML-KEM-768; HQC available as the second KEM |
| Stratum V2 pool protocol | **SHIPPED** | `stratum-v2`, `pool-service` | No chain dependency in the protocol crate, so it fuzzes alone |
| SPV light client | **SHIPPED** | `light-client` | Header fork choice and state-proof verification |
| Network simulation harness | **SHIPPED** | `src/network/sim.rs` | Latency, packet loss and partition modelling. The loss dial arrived with the radio transport: a transport whose whole problem is erasure cannot be tested by a harness that models none |
| eBPF/XDP zero-copy driver (`aya`) | **PLANNED** | — | Kernel-bypass packet path for relay nodes. No code. (Grepping for `aya` here matches the project name — it is not a dependency) |
| LoRa off-grid transport | **RESEARCH** | `radio-transport` | ISM-band header relay for regions with no IP transit. Carries **headers and SPV proofs only**: a hybrid signature is 11,165 bytes and incompressible, so a transaction is 60 frames and forty minutes of duty cycle at best. Chain-free, so its frame decoder fuzzes alone |
| Fountain-coded fragmentation | **RESEARCH** | `radio-transport/src/fountain.rs` | Rateless erasure coding over a window of headers. A 1% duty cycle makes retransmission cost another window, so loss is answered by emitting more symbols rather than by asking again — there is no reverse path to ask on |
| Store-and-forward mesh relay | **RESEARCH** | `radio-transport/src/relay.rs` | Custody, TTL and replay-safe dedup, until a node with IP transit is reached. A relayed header takes the identical path to one off TCP — see the note below, which this subsystem is the first real test of |
| Satellite uplink | **PLANNED** | — | Same fragmenter, a different link MTU and no duty cycle. Deferred until the terrestrial path is real |
| LEO free-space laser mesh | **PLANNED** | — | Optical inter-satellite links |
| CCSDS delay-tolerant networking (BPv7) | **PLANNED** | — | Bundle Protocol v7 store-and-forward, for links where round-trip time exceeds any sane timeout |
| Subsea acoustic signalling | **PLANNED** | — | |
| Subterranean neutrino signalling | **PLANNED** | — | |

A note on the transports above: none of them may change consensus. A block is a
block regardless of the medium that carried it, and the chain must never
acquire a rule that depends on *how* a message arrived — that would make the
transport layer consensus-critical and hand an attacker a fork by radio.

## 4. AI and hardware integration

| Component | Status | Where | Notes |
|---|---|---|---|
| zkML proof verification (halo2, BN254) | **RESEARCH** | `zkml` | Hand-written circuit, verifier only. Dark — see §2 |
| ONNX import, key generation, proving | **RESEARCH** | `zkml-prover` | Off-chain only. A separate crate, not a feature, so the node's graph cannot reach `tract` — invariant 20 |
| GPU mining (CUDA) | **SHIPPED** | `cuda-miner` | `cuda` feature off by default — invariant 3 |
| GPU mining (wgpu: Vulkan/Metal/DX12) | **SHIPPED** | `wgpu-miner` | [wgpu-miner.md](wgpu-miner.md) |
| Confidential federated AI in TEE enclaves | **PLANNED** | — | SGX / SEV-SNP. No code. Note that a TEE attestation is a *vendor's* signature, which is a trusted party — introducing one is invariant-11-shaped and gets written down |
| TPM 2.0 / PUF hardware attestation | **PLANNED** | — | |
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
| Kani model checking | **SHIPPED** | `ledger-math`, `dex`, `governance`, `fee-market` | Which is why those crates stay dependency-free — invariants 1, 6 |
| Supply-chain gates | **SHIPPED** | `deny.toml`, `cargo audit` | Committed, run in CI |
| Telemetry threat surface | **SHIPPED** | `telemetry` | Nothing on the dashboard is verified, and that is stated rather than papered over — invariants 14, 15 |
| State invariant guard + circuit breaker | **SHIPPED** | `src/state/invariant_guard/` | Value conservation over all five holding places fails the block; an anomaly halts one module for 100 blocks and never a transfer — invariant 28, [invariant-guard.md](invariant-guard.md) |
| Exploit replay suite | **SHIPPED** | `tests/exploit_replays.rs` | Re-entrancy, overflow and flash-loan replays. Two of the three have no surface on this chain, which the suite demonstrates rather than assumes |
| LibAFL dynamic fuzzer | **PLANNED** | — | Would replace or sit beside the cargo-fuzz targets with a custom, coverage-guided harness |
| ZK-SIEM threat mesh | **PLANNED** | — | Cross-node intrusion signal without revealing what was observed |
| Self-synthesizing bytecode hot-patcher | **PLANNED** | — | **Reconcile with invariant 13 before any code is written.** No governance key's value is a program, and native code is never fetched from chain state and run. A hot-patcher that takes its patch from the chain violates that outright; one that selects between implementations the binary already ships does not |
| ISO 20022 XML messaging parser | **RESEARCH** | `iso20022` | Bank-rail interoperability: pacs.008, pacs.009, camt.053. Chain-free like `stratum-v2`, so the decoder of untrusted XML fuzzes alone — `fuzz/fuzz_targets/iso20022_decode.rs`, landed with the crate rather than after it. Entity expansion is off: XXE and entity-expansion bombs are the class a bank-rail parser meets first |
| ISO 20022 → L1 bridge | **RESEARCH** | `iso20022/src/bridge.rs`, `api-gateway` | Translates a payment instruction into a sealed `TxKind` and renders camt.053 back out of committed state. Refuses mainnet, the way `state::zkml::check_setup` does, because the envelope's confidentiality is classical and envelopes are on chain forever. The gateway signs for payments that arrive with no Maya2C key, which makes it the chain's second trusted party after the oracle — absent by default |
| Sanctions non-membership proofs | **RESEARCH** | `zk-privacy/src/sanctions.rs`, `iso20022/src/sanctions.rs` | Proves a party is *not* on a published list without revealing who they are, so a compliance check costs no anonymity. The circuit sits in `zk-privacy`, which already owns the field, the Poseidon hash and the tree; `iso20022` holds only the identifier encoding, so its XML decoder never pulls arkworks into a fuzz target |
| Real-world asset primitives | **RESEARCH** | `rwa`, `src/state/rwa.rs` | `RwaToken`, a paged `OwnershipCapTable`, `LegalAttestation`, `RevenueDistribution`. Chain-free records; the cap table is paged like the revocation bitmap because a single unbounded one is a record nobody wrote a limit for |
| Atomic delivery-versus-payment | **RESEARCH** | `src/state/rwa_exec.rs` | Both legs or neither — and a settlement that cannot complete is a **no-op**, never an `Err`. Invariant 7: a failing transaction fails its whole block here, so a DvP that errored would hand anyone a way to void a block |
| Jurisdictional transfer rules | **RESEARCH** | `src/state/rwa_exec.rs` | An issuer names a claim schema, a predicate and trusted credential issuers; the binary evaluates. Invariant 13 — no governed value is a program, so an issuer selects a rule rather than supplying one. Eligibility is proved once and cached in `r:elig:`, because a Groth16 check per transfer is 10-20 seconds on a ten-thousand-transfer block |
| Pro-rata revenue distribution | **RESEARCH** | `ledger-math::distribute` | Largest remainder, tie-broken by holder index. Kani-checked that the payouts sum to exactly the total — a rounding rule that lost a base unit would be refused by the invariant guard, so it is a correctness property rather than a fairness one |
| ZK dark pools | **PLANNED** | — | Would build on `dex` and `zk-privacy`. Invariant 7 applies: a trade that merely loses is a no-op |
| Relativistic gravitational clock synchronisation | **PLANNED** | — | **Note invariant 9.** Oracle freshness is measured in block height, never in timestamps, because `header.timestamp` is miner-chosen and unbounded. A better clock does not change that — it would have to arrive with a rule bounding what a miner may write |

## 6. Tooling and formal verification

| Component | Status | Where | Notes |
|---|---|---|---|
| Tauri 2.0 desktop wallet | **SHIPPED** | `wallet-gui/` | Plus the CLI wallet in `wallet` |
| Leptos block explorer and dashboard | **SHIPPED** | `explorer`, `dashboard/` | `dashboard/` is not a workspace member — CSR Leptos is `wasm32` only |
| Axum REST/GraphQL gateway | **SHIPPED** | `api-gateway` | Talks JSON-RPC to a node, never opens the state database |
| Multi-language SDKs | **SHIPPED** | `sdk-ffi`, `sdk-wasm`, `sdk-js` | Which bindings are generated but never compiled: [sdk.md](sdk.md) |
| LaTeX technical reference generator | **SHIPPED** | `docgen` | Generated from module documentation |
| Testnet faucet | **SHIPPED** | `faucet` | Two independent rate-limit buckets — invariant 16 |
| Lean 4 mathematical proof engine | **PLANNED** | — | Machine-checked proofs of the ledger and consensus rules. Kani covers the arithmetic crates today; Lean would reach what bounded model checking cannot |

## 7. Execution directives

Four standing rules. Each is stated with its *current* enforcement, because a
directive documented as achieved is a directive nobody will implement.

**Zeroization of cryptographic memory.** `zeroize` is a dependency of every
crate that holds key material: root, `crypto-pq`, `custody-mpc`, `mev`, `vrf`,
`wallet`, `wallet-gui/core`, `wallet-gui/src-tauri`, `app-maya2c`. New secret
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

**No `unsafe` in core execution paths.** Holds for `src/state/`, `src/chain.rs`,
`ledger-math`, `dex`, `governance`, `fee-market`, `vrf` — the crates whose
dependency-freedom exists precisely so they can be model-checked. Three crates
carry `unsafe` by construction and are exempt: `cuda-miner` (FFI to the CUDA
driver), `sdk-ffi` (the C ABI is the product), and `wgpu-miner` (GPU buffer
mapping). An `unsafe` block outside those three needs a hardware-attestation
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
