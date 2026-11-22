# Crypto watch

What Maya2C's cryptography depends on, what could change underneath it, and
what the chain does about each change. Master Prompt 15 §6.

> **Status of this page:** written 2026-09-26 from knowledge current to mid-2026.
> No standards body was consulted live while writing it. Every "status" cell is
> a claim to re-check at the next review, not a fact this repository verified.

## 1. What the chain uses

| Use | Primitive | Standard | Where |
|---|---|---|---|
| Transaction signature, lattice half | ML-DSA-65 | FIPS 204 (final, Aug 2024) | `fips204` crate (node), `ml-dsa` crate (`crates/crypto-pq`) |
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

A header says which hash it is under: the high nibble of its version byte is
a hash id (`CON-16`, ADR-143). 0 is BLAKE3, and any other value is refused
until a build defines it, so a migration gives the new hash an id and
schedules it, and no header is ever read under the wrong one. Txids and
addresses need no id of their own, because the signed wire version (`TX-6`)
and the suite already choose their hash.

## 5. Domain separation of signed messages

The hybrid suite (`0x30`) signs with an empty FIPS 204 / FIPS 205 context
string, the same bytes the node's own hybrid produces (invariant 30). What
keeps a signature made for one purpose from verifying as another is the
message, not the context, so two rules apply to every caller:

- Every signed message starts with a domain string of its own, and no domain
  is a prefix of another. A new message type gets a new domain.
- One key signs under one suite. A key is never reused across suites.

Adding a non-empty context string to suite `0x30` would change its bytes and
break invariant 30; it needs an ADR and a new suite id, not an edit.

## 6. Implementations in the signature path

Every crate below decides whether a signature is valid, so a version change
is a consensus change. Each is pinned with `=`; changing one is a reviewed
manifest edit, never a `cargo update`.

| Crate | Pinned | Where | Line |
|---|---|---|---|
| `fips204` | `=0.4.6` | root `Cargo.toml` | pre-1.0 |
| `ml-dsa` | `=0.1.2` | `crates/crypto-pq/Cargo.toml` | pre-1.0 |
| `slh-dsa` | `=0.2.0-rc.5` | `crates/crypto-pq/Cargo.toml` | release candidate |
| `ml-kem` | `=0.3.4` | `crates/crypto-pq/Cargo.toml` | pre-1.0, transport only |
| `x-wing` | `=0.1.1` | `crates/crypto-pq/Cargo.toml` | pre-1.0, transport only |

No published third-party audit of these crates is recorded in this
repository. What stands in for one today:

- **Known-answer vectors:** `crates/crypto-pq/tests/acvp_tests.rs` (NIST ACVP).
- **Two implementations, one answer:** the node verifies the lattice half with
  `fips204`, suite `0x30` with `ml-dsa`. `crates/node/tests/suite_parity_tests.rs`
  pins identical keys and signatures; `crates/node/tests/verify_differential_tests.rs`
  pins identical verdicts on corrupted signatures, corrupted keys and altered
  messages. A disagreement there is a disagreement between nodes.
- **Advisories:** `cargo deny` fails the build on a RustSEC entry.
  `cargo vet` runs report-only (`security-audit.yml`), so it is not a gate yet.

**The `slh-dsa` release candidate.** The hash-based half of every hybrid
signature runs on `0.2.0-rc.5`. The exact pin means no build picks up a
different candidate silently; the ACVP vectors and the parity tests run against
the pinned version. Checked 2026-11-22 against deps.dev and OSV: no final
`0.2.0` is published (the latest stable release is `0.1.0`, 2024-08-18), and
OSV lists no advisory for `0.2.0-rc.5`. Check again before genesis. If a final
release is adopted, re-run the vectors and both parity files before merging the
pin change. Moving it after genesis is a scheduled upgrade, not a patch.
