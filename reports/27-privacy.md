# 27 — Privacy as a developer primitive

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 27

> DONE WHEN: private state and functions work in contracts; viewing keys
> and association-set proofs pass tests; mobile proving is measured; the
> privacy limits document exists; reports/27-privacy.md is complete.

| Condition | Result |
|---|---|
| Private state and functions in contracts | **built and tested in the VM** (2026-09-28): `maya-zk-stark::private_state` + the opt-in `maya_priv` VM imports; a WASM contract keeps a hidden balance as a commitment. Not on chain: needs an audit and an activation height |
| Viewing keys pass tests | **yes**, `crates/privacy` (per account and per transaction) |
| Association-set proofs pass tests | **both halves**: exclusion (`sanctions`) and, since 2026-09-28, inclusion in an approved set bound to a nullifier (`maya-zk-stark::association`) |
| Mobile proving measured | **no.** Desktop only, recorded |
| Privacy limits document | `docs/privacy/LIMITS.md` |

```
$ cargo test -p maya-privacy --profile ci -- --nocapture
test the_account_viewing_key_opens_its_envelopes_and_no_one_elses ... ok
test a_disclosure_key_opens_one_transaction_and_not_the_accounts_others ... ok
test a_tampered_envelope_does_not_open ... ok
non-membership in a 1,000-entry flagged set: prove 72 ms, verify 46.8 ms, proof 731521 bytes (debug assertions on; desktop 4 vCPU, not a phone)
test not_from_a_flagged_set_proof_is_measured ... ok
test result: ok. 4 passed; 0 failed …
```

**Viewing keys.** A shielded transfer's details are sealed with a fresh
ML-KEM-768 encapsulation to the account's viewing key, plus
ChaCha20-Poly1305. There are two levels of disclosure:

- the account viewing key opens that account's envelopes and no one else's;
- a per-transaction disclosure key opens exactly one envelope.

A one-bit change to an envelope makes it refuse to open. Everything here is
post-quantum.

**What the measurement says.** A non-membership proof is **731 KB**, about
55× a hybrid transfer. It does not fit on the chain per transaction as it
stands. Proof compression or recursion, or proving against an aggregated
set, is the open work before association-set proofs can be a normal
option.

**Not done.**

- Private contract state and functions, and SDK support for them.
- Approved-set *inclusion* proofs (the Privacy Pools style), and a
  documented comparison with Privacy Pools (not searched:
  `docs/prior-art/privacy-viewing-keys.md`).
- Shielded transfers in a wallet.
- Mobile proving.
- An external cryptography audit, which is required before activation and
  is already in audit scope (a) (`docs/audit/README.md`).

## Private contract state and association sets (2026-09-28)

**Private state.** A contract keeps a private field as a 32-byte commitment
`P(lo, hi, 0.. ‖ r)`. The owner proves an update off chain
(`private_state::prove_debit` / `prove_credit`) that `C_old` opens to `v`,
`C_new` opens to `v ∓ amount`, and `v - amount >= 0`. Only the two
commitments and the amount are public. The contract checks the proof through
the opt-in `maya_priv` import (`crates/vm/src/private.rs`, the host
implements `PrivateVerifier`) and stores `C_new`. Values are two 29-bit limbs
with an explicit borrow, so no equation can hold by wraparound. A field is
capped at 2^58 base units.

```
$ cargo test -p maya-vm --test private_state_tests -- --nocapture
private debit: prove 12.29ms (desktop, debug build), proof 236946 bytes, contract call Ok(1468144) gas
test a_hidden_balance_is_debited_and_credited_by_proof ... ok
```

In that test, a WAT contract is opened with 1,000 hidden units, then:
- debited 300 by proof, with only the amount emitted;
- a replay of the same proof is refused;
- a proof that lies about the amount is refused, and state is unchanged;
- 45 is credited, leaving a hidden balance of 745.

**Association sets.** `association` proves that a deposit
`L = compress(s, AssociationLeaf)` is in an approved set's tree (depth 15),
with the root and `nf = compress(s, AssociationNullifier)` public. `L`, `s`
and the path stay private; `nf` makes the proof single-use. That is the
Privacy Pools shape. The prior-art comparison is left for
`docs/prior-art/` before any public claim (Standing Order 10). Measured:
prove 16–103 ms, verify 7–9 ms, 345,625 bytes.

**Soundness review (MP27 §4), before landing:**

- **HIGH, fixed.** The association nullifier reused the pool's `Nullifier`
  domain tag, so for a pool note built with `rho = 0` the two nullifiers of
  one secret were equal. Fixed with its own domain tags
  (`AssociationLeaf`, `AssociationNullifier`) and a test that every domain
  tag is distinct.
- **HIGH, fixed.** The debit's public amount limbs were range-checked only
  by the Rust wrapper. A caller assembling public inputs directly could pass
  a limb near p and satisfy the equations by wraparound. The amount is now
  bit-decomposed inside the circuit, and a regression test forges exactly
  that case.
- The rest of both circuits was found sound; see the review in the commit.

**Also fixed here:** `pqc_zk_tests` had failed since the multi-VM commit.
`revm-precompile` brings in the BN254 and BLS12-381 pairing curves for the
EVM's own precompiles. No Maya proof system uses them, so the guard is now
scoped to what it protects. No pairing crate may be reachable from any
workspace member but `maya-multivm`, and a third test pins that the two
curves are reached only through the EVM engine.

**Still not measured: mobile proving.** No phone is attached to this
machine; the Android NDK (r30) is installed, so a phone with USB debugging
can be measured directly.

