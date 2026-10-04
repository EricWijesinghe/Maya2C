# External audit packets

Master Prompt 20 §1. At least two independent firms, four scopes. Each packet
lists what the auditor receives. No audit has been commissioned; engaging a
firm costs money and needs "APPROVED: audit-<scope>".

Every packet includes the common base:

- `spec/` v0.1.0 and `spec/README.md` (how the vectors are made)
- `THREAT_MODEL.md` (STRIDE per component), `SECURITY.md`
- [KNOWN_ISSUES.md](KNOWN_ISSUES.md)
- build and test: `CLAUDE.md` § Build & Test, `docs/build.md`, and
  `cargo test --workspace --profile ci`
- the conformance suite (`spec/tests/`, `crates/node/tests/conformance.rs`,
  `node spec/verifier-ts/verify.ts`)
- `features.toml` and `docs/architecture-vision.md` for what is REAL,
  RESEARCH or SIM
- the findings tracker, [FINDINGS.md](FINDINGS.md)

## (a) Cryptography and the post-quantum implementation, including side channels

| Receives | Why |
|---|---|
| `crates/crypto-pq`, `crates/fips204`, `crates/node/src/crypto/hybrid.rs` | the signature paths every transaction takes |
| `crates/crypto-pq/tests/acvp_tests.rs`, `kem_kat_tests.rs` | NIST vectors the implementation passes |
| `reports/dudect.txt`, `reports/02-crypto.md` | constant-time measurements taken so far |
| `crates/signer/src/channel.rs`, ADR-022 | the node–signer AKE (not externally reviewed) |
| `crates/zk-stark`, shielded pool (`CIRCUIT_IS_AUDITED = false`) | deferred until audited (ADR-016) |
| `crates/custody-mpc` threshold-lattice, ADR-014 | our own dealerless keygen |
| HQC, ADR-009 | the known decapsulation leak |

## (b) Consensus, networking and DoS

| Receives | Why |
|---|---|
| `crates/node/src/consensus`, `crates/dag-bft`, ADR-015, ADR-027, ADR-038, ADR-039 | DAG-BFT in the node, attested checkpoints, the n − f quorum |
| `crates/node/src/network` (Noise + ML-KEM handshake, gossip, peer health) | p2p surface |
| `crates/dos-guard`, `reports/16-validator-security.md` | pre-KEM defences and their simulations |
| `fuzz/`, `crates/node/tests/fuzz_harness.rs` (10⁶ inputs, 0 panics) | fuzzing corpus and hours |
| ADR-021 | certificate cost and the 100-validator cap |

## (c) State machine, fees, staking and governance

| Receives | Why |
|---|---|
| `crates/node/src/state`, `spec/03-state.md` | the transfer path and state root |
| `crates/ledger-math`, `crates/dex`, `crates/governance`, `crates/fee-market` | Kani harnesses (`src/proofs.rs`), not run in this session |
| `formal/lean`, `formal/z3`, `crates/fee-market/tests/lean_differential.rs` | proofs and their assumptions (`reports/08-security.md` §2) |
| `crates/treasury`, `docs/ECONOMIC_SECURITY.md` | treasury and vesting rules |
| `docs/invariants.md` | 31 numbered invariants and their pinning tests |
| staking | **not built**; the scope waits for it |

## (d) Node operations and key management

| Receives | Why |
|---|---|
| `crates/signer`, `bins/maya2c-signer`, ADR-022 | keystore, slashing protection, interchange |
| `deploy-production.sh`, `scripts/configure_environment.sh`, `infra/` | deploy path and secret handling |
| `docs/runbooks/`, `docs/SLO.md`, `infra/grafana/` | operations |
| `crates/build-guard`, `cargo xtask release-check` | the production-build guard |
| `crates/node/src/upgrade.rs`, `reports/15-spec.md` | upgrade halt |
