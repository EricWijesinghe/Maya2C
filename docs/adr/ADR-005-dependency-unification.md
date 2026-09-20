# ADR-005: One version per dependency, and the duplication that remains

**Status:** Accepted
**Date:** 2026-09-20

## Context

At commit `223fe56` the workspace had **95 external dependencies across 45
manifests and no `[workspace.dependencies]` table**. Each crate pinned its own
versions. Six dependencies were declared at mutually incompatible versions, so
the lockfile carried two copies of each:

| dependency | versions | split across |
|---|---|---|
| `chacha20poly1305` | 0.10 / 0.11 | node, `mev`, `custody-mpc`, `confidential-ai`, `ebpf-net` vs `wallet`, `wallet-gui/core` |
| `sha3` | 0.10 / 0.11 | `iot-anchor` vs `htlc-lattice`, `stateless-core` |
| `rand_core` | 0.6 / 0.10 | `zkml`, `iot-anchor` vs `crypto-pq` |
| `rand` | 0.8 / 0.10 | `zkml-prover` vs `crypto-pq` |
| `criterion` | 0.5 / 0.7 | `vm` vs node, `htlc-lattice`, `zkml-prover` |
| `blake3` | 1 / 1.8 | 5 crates vs 13 (semver-compatible; one copy in the lock) |

The `sha3` split is the one with teeth: `iot-anchor`, `htlc-lattice` and
`stateless-core` are all Kani-verified, and they were hashing with two
different releases of the same SHAKE implementation.

## Decision

Two rules, so that reading a member manifest still tells you what that crate
uses:

1. **`[workspace.dependencies]` carries the version only.** Feature lists stay
   in the member that needs them, because which features a crate turns on is a
   fact about that crate. Features on an inherited dependency are additive, so
   this composes.
2. **`default-features` appears at the workspace level only where every member
   agrees it is off.** Where one member disagrees it keeps a literal
   declaration with a comment, rather than forcing the other eleven to restate
   a default.

A dependency used by exactly one member stays in that member. Centralising 54
single-use versions would centralise nothing and would hide them from the
crate that owns them.

One deliberate exception to rule 1: `fips204` carries
`default-features = false, features = ["ml-dsa-65"]` in the workspace table,
because that *is* critical invariant 4. Stating it once makes it a
workspace-wide property instead of four manifests that happen to agree.

**Three splits closed:** `blake3` → `1.8`, `criterion` → `0.7`, `sha3` →
`0.11`. The `sha3` bump was verified rather than assumed: `iot-anchor`'s 13
tests pass on 0.11 and it still builds for `thumbv8m.main-none-eabihf` and
`riscv32imc-unknown-none-elf`.

**Three left split, on purpose:**

- `chacha20poly1305` — unifying either direction rewrites AEAD call sites in
  crypto code, which is not a workspace-layout change. The two use sites never
  exchange ciphertext.
- `rand` and `rand_core` — `crypto-pq` is on the 0.10 generation because its
  ecosystem is; `zkml`, `zkml-prover` and `iot-anchor` are on 0.6/0.8 because
  theirs are (halo2, tract, `digest` 0.10). A major-version split with a
  reason is not drift.

## The duplication this does not fix

`cargo tree -d --workspace` reports **110 duplicated version groups**, and
closing the three first-party splits did not change that number. Two findings
worth recording so they are not rediscovered:

**`sha3` still has three majors in the lockfile.** 0.10 arrives through
`fips204` itself and 0.12 through `hqc-kem`. No first-party declaration can
remove them.

**`curve25519-dalek` and `ed25519-dalek` are each in the lockfile twice.**

```
$ cargo tree --workspace -i curve25519-dalek@5.0.0 --edges normal
curve25519-dalek v5.0.0
`-- ed25519-dalek v3.0.0
    `-- libp2p-identity v0.3.0
        `-- libp2p v0.57.0
            `-- custom-l1-node v0.1.0
```

`Cargo.toml` pins `curve25519-dalek = "4"` with the comment *"two copies of a
curve in one lockfile is two sets of encoding rules"*, and pins
`ed25519-dalek = "2.2"` so that `src/state/threat_exec.rs` calls a known
`verify_strict`. libp2p 0.57 pulls `ed25519-dalek 3.0` and with it
`curve25519-dalek 5.0`, so **the consensus-side verification of a gossip
signature runs a different release of the library than the one libp2p used to
produce and check that signature.**

That is not a bug today — ed25519 strict verification is specified behaviour,
not an implementation detail, and `threat-intel` is dark
(`THREAT_INTEL_ACTIVATION_HEIGHT = u64::MAX`). It is exactly the situation the
comment was written to prevent, and it should be closed before that activation
height ever moves.

## Alternatives considered

**Put all 95 dependencies in the workspace table.** Rejected: a dependency
used once is already single-versioned by construction, and hoisting it moves
its rationale away from the only crate that can explain it.

**Unify `chacha20poly1305` as part of this change.** Rejected: it edits AEAD
call sites in `mev`, `custody-mpc`, `confidential-ai` and `ebpf-net` — crypto
code — inside a commit whose subject is manifest layout. A failure there would
be attributed to the wrong change.

**Drop `ed25519-dalek 2.2` and use libp2p's 3.0 transitively.** Rejected here
because depending on a transitive version is how the split arose; the fix is
to move the direct pin to 3.0 deliberately, with the `verify_strict` semantics
re-checked, and that belongs in a change that touches `threat_exec.rs`.

## Consequences

- Adding a dependency that another member already uses now means editing two
  files. Worth it: the alternative is what §7 of `reports/00-inventory.md`
  measured.
- `cargo deny check bans` becomes a meaningful gate for first-party
  declarations, and a noisy one for transitive duplication. CI runs it; the
  110 transitive groups are not treated as failures.

## Revisit when

`THREAT_INTEL_ACTIVATION_HEIGHT` is proposed for a real height — the
`ed25519-dalek` split must be closed first — or when libp2p's own
`ed25519-dalek` reaches the version this tree pins directly.
