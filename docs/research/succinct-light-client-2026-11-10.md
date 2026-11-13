# Succinct post-quantum chain-state proofs for phones — design note

Date: 2026-11-10. Status: RESEARCH note; changes no code, promotes nothing.
Every Maya2C statement cites a file read on that date. Cells that could not be
sourced say MISSING.

## 0. Findings that change the question

1. **The light client cannot follow today's consensus.** `crates/light-client/src/lib.rs:9-22`
   verifies proof-of-work headers and picks by cumulative work. Under DAG-BFT
   (ADR-027 §1) `difficulty_target` is constant and no work is verified. The
   only finality evidence is a quorum `Checkpoint` of attestations
   (`crates/node/src/consensus/bft/attest.rs:173`), exposed over RPC as
   `CheckpointInfo` (`crates/node/src/rpc/types.rs:135`). `crates/light-client`
   never verifies a checkpoint. The first gap to Mina is not a missing STARK:
   it is that no finality-based light client exists.
2. **The committee signs with ML-DSA-87 only** (`attest.rs:69-82`, ADR-051 option 3;
   4,627 B signature, 2,592 B key — `crates/crypto-pq/src/suite/ml_dsa.rs:24-26`).
   The ML-DSA-65+SLH-DSA hybrid is the account-transaction suite.
3. **Native quorum certificates are already near proof size.** `MAX_COMMITTEE = 100`
   (`crates/node/src/consensus/bft/store.rs:66`).

   | Committee | Quorum (n−f) | Signature bytes | Committee keys |
   |---|---|---|---|
   | 7 | 5 | 23,135 B | 18,144 B |
   | 100 | 67 | 310,009 B | 259,200 B |

   Published hash-based proofs are ~200–350 KiB, so a per-checkpoint STARK wins
   on size only above ~65 signers. The value of a STARK is **recursion** —
   compressing a chain of epoch hand-offs into one object.
4. **Succinctness does not remove weak subjectivity.** Keep a trusted period
   shorter than the unbonding period (CometBFT light-client spec).

## 1. Options (published figures; phone verify cost MISSING for all)

| # | Option | PQ? | Size | Prover | Verifier | Source / grade |
|---|---|---|---|---|---|---|
| 1 | Native quorum certificate (status quo) | Yes | n−f × 4,627 B + keys | none | n−f ML-DSA-87 verifies (bench exists: `crates/crypto-pq/benches/certificate.rs`) | repo constants, high |
| 2 | Ethereum sync committee (Altair) | No (BLS) | header + branches + 1 aggregate sig | none | 1 BLS verify | [spec](https://raw.githubusercontent.com/ethereum/consensus-specs/master/specs/altair/light-client/sync-protocol.md) |
| 3 | CometBFT skipping verification | if sigs are | O(n) sigs | none | O(n) | [spec](https://github.com/cometbft/cometbft/blob/main/spec/light-client/verification/README.md) |
| 4 | Celestia header sync + DAS | hash DA | Merkle samples | none | availability only | [docs](https://docs.celestia.org/learn/celestia-101/data-availability); Maya2C analogue `crates/da/src/lib.rs` (RESEARCH) |
| 5 | Mina Pickles/Kimchi | No (Pasta curves, IPA — discrete log) | ~22 KB | — | — | [blog](https://minaprotocol.com/blog/kimchi-the-latest-update-to-minas-proof-system), [Pickles](https://o1-labs.github.io/proof-systems/specs/pickles); PQ status inferred |
| 6 | STARK aggregation of hash sigs (leanMultisig) | Yes | 247.6 KiB (1,400 XMSS); 347.9 KiB (500 SPHINCS); 2→1 recursion 321.3 KiB | 1,475 XMSS/s, M4 Max | 6.3–9.4 ms laptop | [README](https://github.com/leanEthereum/leanMultisig), self-benchmark, not production |
| 7 | RISC Zero verifying SLH-DSA-SHA2-128s | Yes (STARK layers) | 218 KiB (1) … 454 KiB (512) | ~3.1 s/sig RTX 5090 | ~12–15 ms | [bitcoindev 2026-04-13](https://groups.google.com/g/bitcoindev/c/0IdqdnlC4Og), single-author PoC |
| 7b | RISC Zero succinct receipt | Yes (not the Groth16 wrap) | ~200 kB | MISSING | MISSING | [docs](https://dev.risczero.com/api/recursion) |
| 8 | SP1 compressed | STARK (SNARK wraps not PQ) | MISSING | MISSING | MISSING | [docs](https://docs.succinct.xyz/docs/sp1/generating-proofs/proof-types) |
| 9 | Stwo (Circle STARK, M31) | Yes | MISSING | MISSING | on Starknet mainnet | [L2BEAT](https://l2beat.com/zk-catalog/stwo) |
| 10 | Plonky3 recursion (our family; pinned p3 =0.7.0 in `crates/zk-stark/Cargo.toml`) | Yes | MISSING | MISSING | MISSING | [book](https://plonky3.github.io/Plonky3-recursion/print.html); compatibility with pin MISSING |
| 11 | Lattice folding (LatticeFold+) | plausibly | MISSING | paper claims | MISSING | [ePrint 2025/247](https://eprint.iacr.org/2025/247) |
| 12 | WHIR/STIR commitments | Yes | MISSING | MISSING | ~100s µs | [ePrint 2024/1586](https://eprint.iacr.org/2024/1586) |
| 13 | ML-DSA inside a STARK | — | MISSING | MISSING | MISSING | no published benchmark; repo estimate 10^7–10^8 constraints (`docs/architecture-vision.md:64`) |

No PQ option is near Mina's 22 KB. Honest framing: constant-size, PQ, ~10× larger.

## 2. Candidate designs

- **A. Native finality light client** — trusted checkpoint + genesis BFT validators,
  verify each quorum natively (CometBFT/Altair shape). PQ today, no consensus change.
- **B. Off-chain STARK over epoch hand-offs** — prove C₀→Cₖ linkage; needs a BLAKE3
  Merkle opening in-circuit (`crates/node/src/state/merkle.rs`), cost MISSING.
- **C. Consensus-visible committee commitment** — Poseidon2/Keccak root of the next
  committee (both in `crates/zk-stark/src/hash.rs`), removing BLAKE3 from the circuit.
- **D. STARK-friendly votes** (hash-based aggregatable) — most expensive; stateful OTS
  interacts with the safety log (ADR-027 §4). Not before Option 6 is reproduced.
- **E. Validity proofs** — out of reach on published numbers; stateless witnesses
  (`docs/stateless.md`, `crates/light-client/src/stateless.rs`) stay RESEARCH
  (`STATELESS_ACTIVATION_HEIGHT = u64::MAX`, `crates/node/src/state/context.rs:122`).

## 3. Recommended path

| Phase | Work | Consensus-visible? | Exit evidence |
|---|---|---|---|
| 0 | Finality light client in `crates/light-client`: verify `Checkpoint` quorum, committee tracking from genesis + staking proofs against `state_root` (`LightClient::verify_account`, `chain.rs:279`), trusted-period rule | No | vectors: forged quorum, wrong epoch, stake weight (ADR-040), rotation; ML-DSA-87 verify timing |
| 1 | Phone benchmarks (ML-DSA-87 verify, Plonky3 verify) | No | replace every MISSING phone cell |
| 2 | Off-chain epoch-chain proof prototype (linkage first, then in-circuit ML-DSA-87) | No | prover time/size/verify on stated machine; negative test per constraint |
| 3 | Committee commitment: state record (invariant 25) or header bump (ADR-063); via ADR-082 schedule, ≥ MIN_UPGRADE_LEAD_BLOCKS ahead | Yes | ADR, spec, vectors, invariant, two reviewers |
| 4 | Optional STARK-friendly votes via suite registry (inv. 29, 30) | Yes | reproduced aggregation numbers |
| 5 | Validity proofs | Yes | after signature-in-STARK cost is measured |

## 4. Milestones

M0 native finality light client · M1 phone bench harness · M2 BLAKE3/Poseidon2
Merkle-open cost in `crates/zk-stark/src/gadgets` · M3 in-circuit ML-DSA-87 cost ·
M4 recursion vs pinned Plonky3 · M5 epoch-chain proof · M6 committee-commitment ADR ·
M7 decision gate (no proof if native is cheaper on a phone) · M8 validity-proof
feasibility · M9 prior-art entry before any public comparison with Mina.

## 5. Confidence

| Claim | Confidence |
|---|---|
| Light client cannot follow BFT finality today | High |
| Crossover near ~65 signers | Moderate |
| Hash-based recursive proofs 200–350 KiB, ms verify | Moderate-low (self-reported) |
| Mina not PQ | Moderate (inferred from construction) |
| ML-DSA-in-STARK cost | None |
| Phone verify cost | None |
