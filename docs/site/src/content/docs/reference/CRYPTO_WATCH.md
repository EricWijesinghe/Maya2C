---
title: 'Crypto watch'
editUrl: false
# GENERATED from docs/CRYPTO_WATCH.md by scripts/ingest.mjs. Edit the source, not this.
---
What Maya2C's cryptography depends on, what could change underneath it, and
what the chain does about each change. Master Prompt 15 §6.

> **Status of this page:** written 2026-09-26 from knowledge current to mid-2026.
> No standards body was consulted live while writing it. Every "status" cell is
> a claim to re-check at the next review, not a fact this repository verified.

## 1. What the chain uses

| Use | Primitive | Standard | Where |
|---|---|---|---|
| Transaction signature, lattice half | ML-DSA-65 | FIPS 204 (final, Aug 2024) | `crates/fips204`, `crates/crypto-pq` |
| Transaction signature, hash-based half | SLH-DSA-SHA2-128s | FIPS 205 (final, Aug 2024) | `crates/crypto-pq` |
| P2P key agreement | ML-KEM-768 inside Noise XX (X25519) | FIPS 203 (final, Aug 2024) | `crates/node/src/network/pq/` |
| Second KEM (optional) | HQC | selected by NIST in 2025; standard in draft | node feature `hqc` (RESEARCH gate) |
| Ids, addresses, state root | BLAKE3 | BLAKE3 specification | everywhere |
| Suite registry (dark) | Ed25519, ML-DSA-44/87, SLH-DSA variants | FIPS 186-5 / 204 / 205 | `crates/crypto-pq/src/suite/` |

## 2. What to watch, and the trigger for each

| Item | Watch for | Action when it happens |
|---|---|---|
| FIPS 206 (FN-DSA / Falcon) | final publication | Evaluate as an additional suite (smaller signatures). Signing uses floating point → wallets only, never consensus; verification is integer-only. Until final it is labelled **draft** and stays out of the registry. |
| HQC standard | final publication | Move the `hqc` feature from RESEARCH to a registry-eligible KEM; re-run KATs against the final text. |
| NIST additional signature on-ramp | round results | Re-evaluate compact signature schemes for vote certificates (Master Prompt 13 §3). |
| Cryptanalysis of module lattices (ML-DSA, ML-KEM) | any published reduction in security category | The hybrid still holds while SLH-DSA stands; start the deprecation lifecycle (§3) for the affected parameter set. |
| Cryptanalysis of SLH-DSA / SHA-2 | any practical attack on the hash assumptions | Same, for the hash-based half. |
| BLAKE3 | any published collision or preimage progress | Start the hash migration (§4). |
| IETF | ML-KEM hybrids in TLS, ML-DSA/SLH-DSA in X.509 and CMS | Align RPC gateway TLS and any certificate tooling; no consensus effect. |
| Implementation advisories | RustSEC entries for `fips204`, `slh-dsa`, `ml-kem`, `blake3` | `cargo deny` fails the build; patch within the incident runbook's timelines. |

Review cadence: quarterly, and within a week of any item above moving.

## 3. Suite deprecation lifecycle

Each step is a governance action with a minimum duration, recorded as a MIP
(`spec/mips/`). Durations are floors; governance may be slower, never faster.

| Step | What changes | Minimum before next step |
|---|---|---|
| 1. Announce | MIP published; wallets and exchanges notified | 90 days |
| 2. New suite available | new suite's activation height reached; both accepted | 180 days |
| 3. Default switch | wallets and SDKs create new accounts under the new suite | 180 days |
| 4. Old suite deprecated | old suite still verifies; RPC and wallets warn on use | 365 days |
| 5. Forced migration window | old-suite accounts can only send a key-rotation transaction to a new-suite key | 180 days |
| 6. Old suite rejected | activation height after which the old suite's signatures are invalid | — |

The mechanism exists today in part: suite-tagged (v7) and multisig (v8)
transactions and the closed suite registry are built and active from genesis
(`SUITE_ENVELOPE_ACTIVATION_HEIGHT = 0`, ADR-013), governance chooses the
default suite from the compiled set, and key rotation
without an address change exists in `crates/smart-account`. Step 5's
rotation-only rule is **not built**.

An emergency (a practical break) compresses steps 1–4 into the forced
migration window alone; the hybrid construction is what makes that survivable,
because an attacker who breaks one half still cannot sign.

## 4. Hash agility

The state commitment and block hashing move to a new hash by re-commitment at
an epoch boundary E:

1. A MIP names the new hash and E. The tree shape does not change
   (ROOT-1 … ROOT-4 with the hash swapped), so no proof format changes.
2. At E every node re-computes its account set under the new hash. All honest
   nodes compute the same root because the inputs are the committed state.
3. The block at E carries a bridge record `H(domain ‖ old_root ‖ new_root)`
   under **both** hashes, so a light client holding a pre-E root moves to the
   new root by checking one record, trusting no server.
4. From E, headers commit the new root; proofs under the old hash no longer
   verify against it.
5. A re-commitment longer than one block interval runs in the background
   during the epoch before E and is checked at E.

Rehearsed on real node state by `crates/node/tests/hash_migration_rehearsal.rs`
(20 nodes agree on the SHA3-256 re-commitment, proofs switch at the boundary,
1,000,000-account re-commitment timed). The node implements **no** second hash;
this is a rehearsed procedure, RESEARCH.

Block ids (`CON-2`) move the same way: a header past E is identified under the
new hash, and the bridge block is identified under both.
