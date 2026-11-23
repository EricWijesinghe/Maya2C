# Pending consensus decisions (2026-11-24)

Four consensus findings from the independent audit change a security policy or
the protocol's economics, so they are not implemented until Eric chooses.
Each lists the options, the evidence, and a recommendation. The node-local
part of CON-05 (keeping the evidence, correcting the operator text) does not
need a decision and ships in the consensus hardening PR.

CT-03 (v8 multisig approvals outside `tx_root`) already has its own draft,
#654; it is not repeated here.

## CON-05: attestation double-signs are detected but cannot be slashed

**Evidence (master 3197da3a).**
`crates/node/src/consensus/bft/driver.rs` (`collect`, `on_attestation`) turns
an attestation equivocation into an operator notice only. The pair is not
written anywhere. The on-chain verifier, `staking_exec.rs`
`verify_equivocation` (about line 240), accepts only two `Propose` frames, so
a validator that signs two blocks at one height loses nothing. Proposal
(vertex) equivocation is slashable through the evidence files in
`bins/maya2c-node/src/bft.rs` and `l1-wallet report-equivocation`.

**Options.**

- **A.** Accept an attestation pair in `ReportEquivocation`, slashed at the same
  `double_sign_slash_bps`. Needs a new evidence encoding (a tag byte so the
  two kinds cannot be confused), spec text in `spec/05-consensus.md`,
  conformance vectors, and an ADR-060 / staking amendment. Changes a
  consensus rule, so it lands at an activation height or in the genesis
  format.
- **B.** As A, at a lower rate (an attestation conflict without a proposal
  conflict is a smaller offence: it can split a checkpoint vote but cannot
  create two DAG vertices).
- **C.** Keep attestations unslashed and say so (the text fix below).

**Recommendation: A.** A checkpoint double-sign is the attack that makes two
conflicting finality certificates possible; it should cost the same as a
proposal double-sign. Every BFT chain with slashing (Cosmos double-sign,
Ethereum attester slashing) punishes vote equivocation, not only proposals.

**Shipped without a decision** (consensus hardening PR): the pair is saved to
`<data-dir>/evidence/attestation-*.hex`, and the operator text no longer says
"slashed" or "evidence held" for attestations.

## CON-06: no per-member cap on consensus weight after genesis

**Evidence.** `crates/node/src/state/staking.rs` `stake_weights` is linear in
each member's total stake, with no cap. `BondMore` and `Delegate`
(`staking_exec.rs`) can move stake onto one operator after launch. The 20%
per-operator cap Eric chose (DECISIONS.md) is checked only at genesis, by
`cargo xtask genesis-check`. One operator past one third of the weight can
halt finality; past two thirds it controls it.

**Options.**

- **A.** Cap each member's weight at 20% of the committee's total inside
  `stake_weights` (integer math: clamp, then redistribute nothing; the cap
  simply bounds one member's share). Deterministic, holds whatever path the
  stake took. Needs a spec rule, vectors and an ADR-040 amendment.
- **B.** Refuse a `BondMore` / `Delegate` that would put a validator over the
  cap. Simpler to explain, but stake can still concentrate through slashing
  of others or through unbonding elsewhere.
- **C.** Keep the cap genesis-only (today).

**Recommendation: A**, together with operator-level grouping later (a cap per
key is bypassed by one operator running several keys; grouping needs an
operator identity, which is a separate design). A is the only option whose
bound holds after every state transition.

## CON-07: the active set can shrink below MIN_COMMITTEE

**Evidence.** `crates/staking/src/epoch.rs` `end_epoch` takes
`ranked().take(max)` with no lower bound. `Staking::unbond`
(`crates/staking/src/ops.rs`) has no floor check, and tombstoning through
`slash_double_sign` removes a member without one. Only the downtime-jail path
is floored. #656 covers the jail and slash budget only.

**Options.**

- **A.** Refuse an unbond that would leave fewer than `MIN_COMMITTEE`
  eligible validators (the validator can unbond once a replacement bonds).
- **B.** Accept the unbond but delay its effect to the first epoch end where a
  replacement exists.
- **For tombstones (either):** keep the slash, but defer removal from the
  active set while the set is at the floor.

**Recommendation: B plus deferred tombstone removal.** A refusal (A) can trap
a validator's stake indefinitely on a small network; a delay keeps the floor
without that. The slash itself is never deferred. Fold into the #656 decision.

## CON-08: a fresh node can start following an unconfirmed tip

**Evidence.** `bins/maya2c-node/src/rejoin.rs` `decide` refuses by default and
starts `Anchor::Unanchored` only with `--allow-unconfirmed-start`. A fresh node
with no local state also starts unanchored without `--trusted-block`, although
the 2026-10-25 mainnet plan made `--trusted-block` mandatory. The flag is
present in `--features production` builds.

**Options.**

- **A.** Compile `--allow-unconfirmed-start` out of production builds and
  refuse a fresh mainnet join without `--trusted-block`.
- **B.** Keep the flag with its warning (today).
- **C.** Require M of N independent sources to agree before an unanchored start.

**Recommendation: A.** Weak-subjectivity checkpoints are how every
proof-of-stake chain stops a long-range attack on a new node; a production
binary should not offer a way around one. C is a later improvement for
operators who cannot obtain a checkpoint out of band.
