# ADR-007: Signature suites, a registry, and a suite-tagged envelope

**Status:** Accepted
**Amended by:** [ADR-013](ADR-013-suite-envelope-activation.md), 2026-09-27 — §7 (activation): the envelope is live from genesis.
**Date:** 2026-09-21

## Context

Every transaction today carries exactly one kind of signature: the hybrid
ML-DSA-65 + SLH-DSA-SHA2-128s pair in `crates/node/src/crypto/hybrid.rs`, with
no algorithm identifier on the wire (`crates/node/src/core/transaction.rs:112`).
Changing the algorithm therefore means changing the encoding, and every
historical transaction was written without a way to say which algorithm signed
it. The crypto brief (Master Prompt 2) asks for crypto-agility: algorithms that
can change later without breaking old data, a governance-selectable default,
a security-level audit, and a migration path off a deprecated suite.

Invariant 4 compiled only `ml-dsa-65` into `fips204`, so that a parameter set
nobody chose could not be selected by accident. The brief asks for ML-DSA-87
as the mainnet default, which that invariant forbids as written.

Sizes, measured from the crates and matching final FIPS 204/205 (not the
drafts — ML-DSA-65 is 3,309 bytes, not the 3,293 of the draft):

| Id | Suite | pk | sig | NIST cat | PQ bits (claimed) |
|---|---|---|---|---|---|
| `0x01` | Ed25519 | 32 | 64 | — | 0 |
| `0x10` | ML-DSA-65 | 1,952 | 3,309 | 3 | 192 |
| `0x11` | ML-DSA-87 | 2,592 | 4,627 | 5 | 256 |
| `0x20` | SLH-DSA-SHA2-128s | 32 | 7,856 | 1 | 128 |
| `0x21` | SLH-DSA-SHAKE-256f | 64 | 49,856 | 5 | 256 |
| `0x30` | ML-DSA-65 + SLH-DSA-SHA2-128s | 1,984 | 11,165 | 1 (min of halves) | 128 |

## Decision

1. **A closed registry in `maya-crypto-pq::suite`.** `SuiteId` is a `#[repr(u8)]`
   enum with exactly the six ids above. Unknown bytes are a decode error, never
   a fallback. Each id has one `SuiteInfo` row: name, sizes, NIST category,
   claimed post-quantum bits, and whether it is permitted on mainnet.
2. **The envelope is `version ‖ suite ‖ len(pk) ‖ pk ‖ len(sig) ‖ sig`**,
   lengths as `u32` little-endian, each checked *exactly* against the
   registry row. Variable-length on the wire, fixed-length per suite.
3. **`0x30` is today's hybrid, byte for byte.** Its public key and signature
   are the concatenation `hybrid.rs` already writes, so every existing
   transaction is a valid `0x30` envelope with a two-byte header prepended. It
   verifies only if **both** halves verify, and both are always checked (no
   short-circuit, so the time taken does not reveal which half failed).
4. **Backends.** RustCrypto `ml-dsa =0.1.1` for both ML-DSA sets, `slh-dsa
   =0.2.0-rc.5` for both SLH-DSA sets, `ed25519-dalek 3.0.0` for Ed25519
   (`verify_strict` only, keys load from PKCS#8). All instantiated inside
   `maya-crypto-pq`, which keeps invariant 2 true. `fips204` stays in the
   Ledger app, which needs `no_std` without a heap, and in the node until the
   envelope activates (below).
5. **Ed25519 is devnet-only.** `SuiteInfo::mainnet_allowed` is false for
   `0x01`; the policy refuses it on any network that is not devnet, and the
   audit flags it (0 post-quantum bits).
6. **Invariant 4 is extended, not rewritten.** It stays true of `fips204`
   (still `ml-dsa-65` only); invariant 29 states the rule for the registry:
   *only parameter sets named in it are compiled into a signature path, and
   every FIPS one has NIST known answers.* Invariants 30 (the `0x30` parity)
   and 31 (v7 dark until activation) record the rest of this decision.
7. **Activation.** The node carries suite-tagged transactions as wire
   version 7 (`crates/node/src/core/suite_tx.rs`) beside the hybrid v5/v6
   frames, with its own signing domain and its own address domain
   (`BLAKE3-derive-key(suite ‖ pk)`), so a v7 account can never collide with
   a hybrid one. `Transaction::verify` refuses every v7 frame; only
   `verify_at` at or past `SUITE_ENVELOPE_ACTIVATION_HEIGHT` — `u64::MAX`
   until somebody writes a height down — under a `SuitePolicy` that lets the
   suite sign can accept one. v5/v6 stay valid forever, and are `0x30`
   envelopes without the header, so history replays unchanged. Governance
   chooses the default through `ParameterKey::DefaultSignatureSuite` (tag 10,
   range `0x10..=0x30`, default ML-DSA-87); the node re-checks the byte names a
   permitted suite, because the governance crate cannot know the registry.

## Alternatives considered

- **`pqcrypto-mldsa` / `pqcrypto-sphincsplus`.** PQClean C behind FFI: a C
  toolchain in the static-musl build and an `unsafe` surface in the
  consensus path. Kept as a documented fallback only.
- **Keep `fips204`, enable `ml-dsa-87`.** Would work. Rejected because two
  ML-DSA implementations would then both be in consensus, and RustCrypto's is
  the one the brief prefers and the one whose ACVP harness is upstream.
- **ed25519-dalek 2.2.** Already in the lock through the node's gossip
  evidence code. 3.0 is the latest release and libp2p 0.57 already pulls it,
  so choosing it adds no new copy of curve25519-dalek.
- **Open registry (suites registered at runtime).** A suite that exists only
  on some nodes is a fork. Closed enum.

## Consequences

- A new suite is a code change plus a registry row plus KAT vectors, never a
  governance transaction. Governance can only *choose among* compiled suites.
- ML-DSA-87 as default costs 1,318 bytes more signature and 640 bytes more
  public key per transaction than ML-DSA-65.
- `ml-dsa 0.1.1` and `slh-dsa 0.2.0-rc.5` are pre-1.0 and pinned exactly; a
  bump is a consensus review, gated by the ACVP tests in
  `crates/crypto-pq/tests/acvp_tests.rs`.
