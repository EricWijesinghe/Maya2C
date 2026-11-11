# ADR-117: Multisig suite diversity — a quorum that no single signature family can meet

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-18, [prior-art/multisig-suite-diversity.md](../prior-art/multisig-suite-diversity.md), [ADR-007](ADR-007-signature-suites.md), [ADR-011](ADR-011-threshold-custody.md), [ADR-013](ADR-013-suite-envelope-activation.md), [ADR-036](ADR-036-chain-bound-signatures.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 29, 30, 31

## Context

**Finding that contradicts the brief.** The parity row reads "mixed-suite
multisig signers (hybrid + SLH-DSA in one m-of-n): **none**". That is wrong
in the code. A v8 multisig policy is already a list of *suite-tagged* keys:
`PolicyKey { suite: SuiteId, public_key }`
(`crates/crypto-pq/src/multisig.rs:101-105`), encoded `m ‖ n ‖ (suite ‖ key)*`
(`multisig.rs:168-177`), and `MultisigPolicy::verify` dispatches each approval
to its own key's suite (`multisig.rs:260`). Every member may be any suite with
`pq_security_bits > 0` (`multisig.rs:272-276`), which includes the hybrid
`0x30` (`suite/mod.rs:178`, 256 bits) and both SLH-DSA sets. The test
committee in `crates/crypto-pq/tests/multisig_tests.rs:38-45` mixes ML-DSA-87,
ML-DSA-65 and SLH-DSA-SHAKE-256f in one policy, and the node verifies a v8
frame through `multisig_tx::verify_at` (`crates/node/src/core/multisig_tx.rs:218-234`),
checking every listed key's suite against the admission schedule at the
height. Mixed-suite signers shipped with wire v8. The row should read
**shipped** (`features.toml:1870`, verified).

What a mixed list does *not* give is **diversity in the quorum**. A 2-of-3
policy of `{ML-DSA-87, ML-DSA-87, SLH-DSA-256f}` is met by the two lattice
keys alone. If ML-DSA were broken, the hash-based member would never be
needed. Nothing in the policy can say "at least one approval from each of
two families". That is the gap this ADR fills, and it is the thing Algorand's
2026 roadmap calls multi-scheme quorums (prior-art file).

Facts the design rests on: `MAX_SIGNERS = 16` (`multisig.rs:35`); approvals
are index-ordered and exactly `m` (`multisig.rs:239-250`); the account address
is the hash of the policy encoding (`multisig_tx.rs:97`), so a new encoding is
a new address, never a migration; wire v8 is the last multisig frame
(`multisig_tx.rs:30`); wire v10 and v11 are claimed by ADR-102 and ADR-106
(`adr-claims.txt`).

## Decision

**A policy may carry a family floor: a map from signature family to the
minimum number of approvals that must come from that family. Verification
counts approvals per family after the suite checks and refuses a quorum that
misses any floor.** The m-of-n rule is unchanged; the floor is an extra
condition, never a substitute.

### Families

A compiled, closed mapping, in `crates/crypto-pq/src/suite/mod.rs` beside the
registry (invariant 29: no new suite, no governance act):

| Family | Suites |
|---|---|
| `Lattice` | `MlDsa65`, `MlDsa87` |
| `Hash` | `SlhDsaSha2_128s`, `SlhDsaShake256f` |
| `Hybrid` | `HybridMlDsa65SlhDsa128s` |

A hybrid approval counts toward **both** `Lattice` and `Hash`: a forger must
break both halves to produce it (`suite/mod.rs:121-124`). `Ed25519` is in no
family; it is already refused as a member (`multisig.rs:28-29`).

### Policy encoding, wire v12

`WIRE_VERSION_MULTISIG_V2 = 12` (claimed in `adr-claims.txt`). The frame is
the v8 frame with the policy encoded as

```
m:u8 ‖ n:u8 ‖ floors:u8 ‖ (family:u8 ‖ min:u8)^floors ‖ (suite:u8 ‖ key)*
```

Construction rules, added to `MultisigPolicy::new` (`multisig.rs:131`):

- `floors <= 2` (`Lattice`, `Hash`; `Hybrid` has no floor of its own because
  it counts in both);
- each `min >= 1` and `min <= m`;
- `sum(min) <= m`, so a quorum can exist;
- each floored family must have at least `min` members in the list, counting
  hybrids, so the floor is meetable without a signer who does not exist.

A violated rule is `MultisigError::Unmeetable`, refused at decode like
`BelowThreshold` today. The v8 decoder is untouched; a v8 policy is a v12
policy with `floors = 0` and keeps its address.

### Verification

`MultisigPolicy::verify` (`multisig.rs:239`) gains one pass after the
signature loop: tally approvals by family (a hybrid increments two counters)
and refuse `MultisigError::FamilyFloor { family, got, min }` when any floor
is short. Deterministic, integer, no new cryptography. `multisig_tx::verify_at`
is unchanged: every member's suite is still checked for admission at the
height first (`multisig_tx.rs:225-230`), so a sunset suite (ADR-082) still
forces migration even when the quorum could avoid it.

### Interaction with suite sunset

If a floored family's suites are all retired by the sunset record, the policy
becomes unmeetable. That is the intended outcome, not a bug: an account
whose security premise is gone must move, which is what `check_admissible`
already enforces for every member. The wallet shows this state before the
sunset lead expires (ADR-086 clear signing).

## Alternatives considered

- **Nothing: mixed lists already exist.** True, and recorded above; but a
  mixed list without a floor is a one-family quorum in disguise.
- **Weighted thresholds (Algorand's "weighted approvals").** More general,
  and more ways to write an unmeetable policy; the two-family floor is what
  a custody control needs today. A weight scheme can be a later version.
- **Require the floor on every multisig.** Breaks every existing v8 account
  and the single-family treasury that chose it on purpose. The floor is
  opt-in per policy.
- **A policy flag bit inside `m`** (bit 7 is free since `m <= 16`). Saves a
  wire version at the cost of an encoding only a comment explains. Rejected.
- **True threshold lattice signing** (ADR-011). Stays RESEARCH; this ADR
  is about the registry-level policy, which works with any compiled suite.

## Consequences

- One new wire version, one family table, two policy rules, one tally pass.
- A v12 account has a new address; nothing migrates and no v8 address
  changes meaning (invariant 30 is untouched: suite `0x30` bytes do not move).
- Wallets and `describe.rs` display the floor beside `m-of-n`.
- `features.toml:1870` note gains "suite diversity (ADR-117)"; the parity
  row is corrected to **shipped (mixed), floor proposed**.

## Invariants touched

- **29**: families are a compiled table over the closed `SuiteId` enum;
  no suite is added.
- **30**: unchanged; hybrid bytes are the same bytes.
- **31**: v12 verifies only through `verify_at` under the constant policy;
  `Transaction::verify` refuses it as it refuses v7 and v8.

## Activation

`FeatureSet::MULTISIG_DIVERSITY = 1 << 24` (claimed in `adr-claims.txt`),
compiled `MULTISIG_DIVERSITY_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**. Before activation a
v12 frame is refused at decode with the version named, as v3 and v4 are
(`transaction.rs:446-460`).

## Test plan

Red tests, `crates/crypto-pq/tests/multisig_tests.rs` and
`crates/node/tests/multisig_tests.rs`:

- `a_lattice_only_quorum_is_refused_when_the_hash_floor_is_one`
- `one_hybrid_approval_meets_both_floors`
- `a_floor_above_the_family_count_is_unmeetable_at_construction`
- `floor_sum_above_m_is_unmeetable_at_construction`
- `a_v8_policy_decodes_as_floors_zero_with_the_same_address`
- `a_v12_frame_before_activation_is_refused_by_name`
- `a_retired_floored_family_makes_the_policy_unmeetable_at_height`
- `every_byte_either_names_a_family_or_is_refused`

Vectors: `spec/02-transactions.md` MS-4..MS-6 (a v12 policy, its address, a
refused one-family quorum). `cargo xtask spec-coverage` must list them.

## Open questions for Eric (economics only)

None. This ADR changes no fee, price or parameter. (Whether the foundation
and treasury multisigs adopt a `Hash >= 1` floor at activation is a key
ceremony decision, not an economics one, and is recorded in
`docs/ceremony/` when made.)
