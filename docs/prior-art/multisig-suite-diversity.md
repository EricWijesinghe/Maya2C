# Prior art: Mixed-suite multisig and suite-diverse quorums

**Search status:** searched 2026-11-12 (WebSearch, 1 query: "Algorand multi-scheme post-quantum multisig Falcon ML-DSA hybrid 2026").

Feature: parity row P-18, [ADR-117](../adr/ADR-117-multisig-suite-diversity.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Algorand (roadmap) | Native multi-scheme m-of-n multisig mixing classical, pure-Falcon and hybrid keys, with weighted approvals described; target end of 2026. Falcon-1024 accounts stated live on mainnet in the v5.0.0 upgrade (the company's own page; a Q3 2026 date in earlier coverage is not reconciled with it). | [algorand.co roadmap post](https://algorand.co/blog/algorand-targets-broad-quantum-resilience-by-2027), [technical roadmap](https://algorand.co/blog/algorand-post-quantum-cryptography-roadmap), [technology page](https://algorand.co/technology/post-quantum) |
| Silence Laboratories (secondary) | Notes on why lattice signatures are hard to threshold-sign, which is why policy-level multisig over independent keys is the practical route. | [blog](https://silencelaboratories.com/blog/pq-signatures-mpc-friendliness-and-necessary-endeavors) |

## What Maya2C has (checked in repo, 2026-11-12)

- **Mixed-suite m-of-n is shipped**: `PolicyKey { suite, public_key }` per member, any post-quantum suite including the hybrid `0x30` and both SLH-DSA sets (`crates/crypto-pq/src/multisig.rs:101-105`, `:272-276`); verification dispatches per key (`:260`); wire v8 in the node (`crates/node/src/core/multisig_tx.rs:30`, `:218-234`); a test committee mixes ML-DSA-87, ML-DSA-65 and SLH-DSA-SHAKE-256f (`crates/crypto-pq/tests/multisig_tests.rs:38-45`). The parity row's "none" is wrong.
- Not present: a rule that the quorum must span signature families; weighted approvals; threshold lattice signing (ADR-011, RESEARCH).

## How the Maya2C version differs

- A **family floor** per policy (`Lattice`, `Hash`; a hybrid counts in both), opt-in, encoded in a new frame (v12) so the address changes and nothing migrates.
- Classical suites are refused as members outright (`multisig.rs:28-29`), where Algorand's design mixes classical keys in.
- No weights: two floors over a closed family table, so an unmeetable policy is refused at construction.

**Verdict: similar exists (Algorand's planned multi-scheme multisig). Mixed suites are already shipped here; differentiate by the family floor only. No "first" claim.**
