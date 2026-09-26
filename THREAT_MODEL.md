# Threat model

STRIDE per component (Master Prompt 8 §8). Each row names the threat, the
mitigation **that exists in this tree** with a `path` to it, and — where there
is none — says so. A mitigation listed here without a path would be a claim
nobody can check, so there are none.

Trust boundaries, outermost first: the public internet → the API gateway and
p2p listener → the node process → RocksDB on local disk → key material. The
wallet, the Ledger device and the custody peers are separate processes with
their own boundaries.

Legend: **S**poofing, **T**ampering, **R**epudiation, **I**nformation
disclosure, **D**enial of service, **E**levation of privilege.

## Transaction and block validation (`crates/node/src/core`, `state`, `consensus`)

| | Threat | Mitigation | Gap |
|---|---|---|---|
| S | Forged sender | Hybrid ML-DSA-65 + SLH-DSA signatures, both must verify; suite envelope checked under a constant policy (`crypto::hybrid`, invariants 29-31) | — |
| T | Malleated tx with two txids | Canonical encoding: `ByteReader::finish` rejects trailing bytes; fuzzed for canonicality (`fuzz/fuzz_targets/tx_decode.rs`) | — |
| T | Miner writes a false `state_root` | `apply_block_journaled` re-executes and refuses a mismatch (invariant 24) | — |
| T | Arithmetic wrap creates value | Checked arithmetic; Kani on `ledger-math`; invariant guard checks conservation each block (invariant 28) | — |
| R | A block producer denies producing a block | PoW header + tx root bind the block to its content | Under DAG-BFT (ADR-015, not wired) votes must be signed; not yet |
| I | — | Transparent chain; shielded pool for amounts (ADR-008, `CIRCUIT_IS_AUDITED = false`) | Shielded pool is not activated until an external audit |
| D | Signature-verification flood | Mempool refuses unfunded senders before any signature check (ADR-013) | No per-peer CPU accounting yet (Master Prompt 16 §4) |
| E | Consensus rule changed by config | `config.toml` cannot name a consensus value (`crates/node/src/config.rs`) | — |

## Peer-to-peer (`crates/node/src/network`)

| | Threat | Mitigation | Gap |
|---|---|---|---|
| S | Impersonated peer | libp2p Noise; optional dual-KEM (ML-KEM + HQC) transport (`network::pq::dual`, docs/pq-transport.md) | Dual-KEM off by default (`dual_kem = "off"`) |
| T | Modified gossip | Messages are signed blocks/txs validated on receipt | — |
| D | Handshake flood (KEM work is expensive) | Peer guard / Byzantine scoring (`docs/peer-health.md`) | No stateless retry cookie before KEM work (Master Prompt 16 §4) |
| D | Eclipse | Bootnodes + Kademlia | No sentry topology, no eclipse test yet (Master Prompt 16 §3-4) |
| I | Validator IP exposure | — | Sentry architecture not built |

## RPC and API gateway (`crates/node/src/rpc`, `crates/api-gateway`)

| | Threat | Mitigation | Gap |
|---|---|---|---|
| S | Unauthenticated miner methods called remotely | Node RPC binds 127.0.0.1 by default; gateway default-deny allowlist excludes miner methods | — |
| D | Request flood | Per-IP token bucket (`rpc::limit`), gateway rate limits | No per-method cost accounting (Master Prompt 14 §5) |
| E | Debug methods in production | — | `production` feature gate not built (Master Prompt 11 §3) |

## WASM VM (`crates/vm`)

| | Threat | Mitigation | Gap |
|---|---|---|---|
| T | Non-determinism forks the chain | Pinned wasmtime `=48.0.1`, no SIMD/threads, NaN canonicalisation, config digest in cache key; gas identical across tiers (`tests/tier_differential_tests.rs`) | — |
| D | Infinite loop / memory bomb | Fuel metering, memory/table/stack limits | — |
| E | Sandbox escape via host function | Closed host function list checked against the linker (`tests/host_surface_tests.rs`) | wasmtime itself is trusted |

## Key custody (`crates/custody-mpc`, `apps/wallet-gui`, `apps/ledger-maya2c`)

| | Threat | Mitigation | Gap |
|---|---|---|---|
| S | Custody peer impersonation | TLS X25519MLKEM768 + channel-bound ML-DSA-87 peer auth (`custody-mpc::pq_auth`) | — |
| I | Key leak via memory / logs | `ZeroizeOnDrop`, `secrecy`, compile-time `!Display` checks (`tests/secret_hygiene_tests.rs`) | — |
| I | Timing side channel | dudect harness (`scripts/dudect.sh`) | **HQC decapsulation leaks (|t| 47.5)**; HQC stays draft and never sole (ADR-009) |
| T | Blind signing | Ledger app displays the transaction pages before signing (`apps/ledger-maya2c`) | Clear-signing intent standard is Master Prompt 22 |

## Supply chain

| | Threat | Mitigation | Gap |
|---|---|---|---|
| T | Malicious dependency | `cargo deny` (advisories, licences, sources), `cargo audit` in CI | `cargo vet` not adopted; reproducible builds not verified on two machines (Master Prompt 16 §5) |
| T | Compromised release binary | — | Signing, SBOM, provenance: Master Prompt 10 §2 (`.github/workflows/production_build.yml`) |

## What this model does not cover

Physical attacks on validator hosts beyond the TPM/tamper design notes;
social engineering of operators; legal compulsion. Each needs an operational
answer (runbooks, key ceremonies), not code.
