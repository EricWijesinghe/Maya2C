# OSTIF — audit request draft

OSTIF (Open Source Technology Improvement Fund, ostif.org) arranges and
funds security audits of open-source software. It takes requests through
its website. This is the text to paste; the owner submits it (ADR-042,
gate 7). Every figure below is measured, with the command beside it.

## Project

**Maya2C** — a post-quantum layer-1 blockchain node in Rust.
Code: https://github.com/EricWijesinghe/Maya2C (Apache-2.0)
Site: https://maya2c.dev · Live testnet: https://status.maya2c.dev

## What it is, in three lines

Every transaction carries two NIST post-quantum signatures: ML-DSA-65 (FIPS
204) and SLH-DSA (FIPS 205). Both must verify. Blocks are finalised by
DAG-BFT (Narwhal and Bullshark style), with stake-weighted quorums. Peers
talk over libp2p with an ML-KEM-768 layer over Noise.

## Why an audit matters now

Mainnet is planned with four project-run validators, to start the record a
public chain needs. It will be labelled "external audit pending" until an
independent report exists. The project has one maintainer and no budget,
so it cannot buy that report. Post-quantum signing in consensus is recent
territory, and a flaw in this code would be paid for by users, not by
the project.

## Scope requested, by priority

Measured with `git ls-files <dir> | rg '\.rs$' | xargs cat | wc -l` on
2026-10-05 (whole tree: 223,062 lines of Rust).

| # | Component | Rust lines | Why it is first |
|---|---|---|---|
| 1 | `crates/crypto-pq`, `crates/node/src/crypto` (hybrid signatures, KEM, constant time) | 5,993 + 3,745 | every transaction and every peer session |
| 2 | `crates/dag-bft`, `crates/node/src/consensus` (engine, attested checkpoints, catch-up, stake weights) | 2,988 + 4,717 | safety and liveness of the chain |
| 3 | `crates/node/src/network` (libp2p, PQ handshake, WebSocket, peer limits) | 7,590 | remote attack surface |
| 4 | `crates/node/src/state` (apply path, state root, staking, slashing) | 15,070 | value accounting |

The shielded pool (`crates/zk-stark`) is **out of scope**: it is disabled
for mainnet v1 (`CIRCUIT_IS_AUDITED = false`).

## What the auditor receives

`docs/audit/README.md` lists one packet per scope, and all four share a
common base:
- threat model (`THREAT_MODEL.md`);
- specification and conformance vectors (`spec/`);
- known issues and findings (`docs/audit/KNOWN_ISSUES.md`,
  `docs/audit/FINDINGS.md`);
- the invariants (`docs/invariants.md`);
- the automated work already done (`docs/security-audit.md`): nightly
  fuzzing of every decoder, Kani proofs of the ledger arithmetic, the
  unsafe-code gate, cargo-deny, CodeQL;
- the self-run attacknet (`cargo xtask attacknet`) and its dated reports
  (`reports/attacknet/`).

## Maintainer

Eric Wijesinghe — sole maintainer. Contact: through GitHub
(@EricWijesinghe) or the address on maya2c.dev.
