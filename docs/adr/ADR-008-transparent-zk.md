# ADR-008: Plonky3 STARKs as the only proof system

**Status:** Accepted
**Date:** 2026-09-21

## Context

Two proof systems were in the tree when the crypto brief arrived, and neither
survives its requirement of "no Groth16, no BN254, no trusted setup":

| Crate | System | Setup | Post-quantum |
|---|---|---|---|
| `zk-privacy` | Groth16 over BLS12-381 (`ark-groth16`) | per-circuit trusted setup | no (pairings) |
| `zkml` / `zkml-prover` | halo2 with KZG over BN254 (`halo2-axiom`) | universal SRS, derived from a public seed so anyone can forge (invariant 22) | no (pairings) |

Both rest on discrete-log hardness in a pairing group. Shor's algorithm breaks
the soundness of every proof — a quantum adversary forges proofs of false
statements — not just the privacy of old ones.

## Decision

1. **One proof system: Plonky3 (`p3-uni-stark =0.7.0`), a hash-based STARK
   with FRI.** It is transparent (no setup of any kind) and its soundness
   rests only on the collision resistance of the hash, which Grover's
   algorithm weakens by at most a square root.
2. **Field and hashes.** BabyBear (p = 2^31 − 2^27 + 1) with its degree-4
   extension for challenges; Poseidon2 inside the AIR (the arithmetic-friendly
   hash that note commitments and Merkle paths use); Keccak-256 for the FRI
   Merkle commitments and the Fiat–Shamir transcript, so the outer layer uses a
   hash with decades of cryptanalysis.
3. **Security target.** FRI parameters are chosen for at least 100 bits of
   *conjectured* soundness: `log_blowup = 2`, 100 queries, 16 bits of query
   grinding — Plonky3's own `new_benchmark_zk` preset — written next to the
   config in `crates/zk-stark/src/config.rs`. The level is computed, not
   asserted: Plonky3's estimator gives the joinsplit AIR **113 bits conjectured
   and 99 bits proven** (`pool::tests::report_proof_size_and_security`; the
   P7a spend AIR measured 114 / 101), bounded
   by the ~124-bit challenge extension and the 128-bit Keccak collision
   resistance. The first draft of this ADR named `log_blowup = 3` with 38
   queries; the measured preset replaced it.
4. **Halo2-IPA is not permitted anywhere.** It would be transparent but not
   post-quantum. No crate in the workspace may use it; nothing is exempted.
5. **What goes.** `ark-groth16`, `ark-bls12-381`, `ark-snark` and the rest of
   arkworks leave `zk-privacy`, which is rebuilt on `maya-zk-stark`.
   `halo2-axiom` and `halo2curves-axiom` leave the workspace with `zkml` and
   `zkml-prover`; the on-chain zkML feature becomes `planned` until it is
   re-expressed as a STARK. It was dark (activation `u64::MAX`, invariant 22)
   and forgeable, so nothing that worked stops working.
6. **The gate.** `crates/zk-stark/tests/pqc_zk_tests.rs` runs `cargo tree
   --workspace -e normal,build,dev` (host, offline) and scans `Cargo.lock` for
   every other target, and fails if `ark-groth16`,
   `ark-bn254`, `ark-bls12-381`, `ark-snark`, any `halo2*`, `bls12_381`,
   `bellman` or `snark-verifier` appears. Dev edges are included because a
   test-only Groth16 would still be a second, weaker proof system in the tree.
7. **The mainnet block moves, it does not lift.** `SETUP_IS_TRUSTED` went with
   `zk-privacy`. Its guards now read `pool::CIRCUIT_IS_AUDITED = false`: there
   is no setup to trust, but an under-constrained AIR has the same consequence
   — hidden supply — and the negative tests only cover constraints this tree
   knows about.

**Done 2026-09-21.** `zk-privacy` is deleted rather than rebuilt: the joinsplit,
credential and sanctions statements live in `zk-stark` itself (`pool/`,
`credential.rs`, `sanctions.rs`), since a second crate would hold only
re-exports. The node verifies joinsplits through `maya_zk_stark::pool::verify`;
the block cap fell from 64 to 16 joinsplits because a proof is ~0.4 MB.

## Alternatives considered

- **Binius.** Post-quantum and fast for binary-field workloads, but its Rust
  implementation has had no stable crates.io release line to pin, and the
  circuits here are arithmetic (Poseidon2, range checks), which is not where
  Binius is strongest.
- **Winterfell.** A mature STARK prover, but single-AIR-per-proof ergonomics
  and a slower field; Plonky3 has the active maintenance and the Poseidon2
  gadgets.
- **Keep Groth16 behind a flag.** A flag can be switched on by any crate in
  the graph through feature unification (the argument of invariant 20); the
  requirement is "nowhere in the workspace".

## Consequences

- Proofs are larger: tens to low hundreds of kilobytes against Groth16's 192
  bytes. Measured sizes go in `reports/02-crypto.md`.
- Invariants 20–23 describe a crate that no longer exists. They are marked
  *retired with zkml* in `docs/invariants.md`; their numbers are not reused.
- Every AIR constraint gets a negative test that tampers with one witness
  column consistently and expects the proof to fail — invariant 23's lesson,
  carried over.
