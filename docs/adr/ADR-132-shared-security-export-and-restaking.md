# ADR-132: Shared-security export and restaking — options for letting Maya2C stake secure anything but Maya2C

**Status:** OPTIONS FOR ERIC, 2026-11-12. Nothing is decided here. Every option but A is outside the ADR-016 launch scope and post-genesis.
**Date:** 2026-11-12
**Related:** parity row P-26, [prior-art/restaking.md](../prior-art/restaking.md), [ADR-028](ADR-028-staking.md), [ADR-016](ADR-016-launch-scope.md), [ADR-030](ADR-030-vault-accounts.md), ADR-096 (exit-window derivation, `postgen/v2`), ADR-133 and ADR-134 (this branch), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 11, 12, 17, 25, 28, 32

## Context

What a Maya2C validator's stake is today:

- **One bond, one duty.** `Params { min_self_bond, min_delegation,
  max_validators, unbonding_epochs, double_sign_slash_bps,
  downtime_slash_bps, downtime_threshold_bps, jail_epochs, max_commission_bps }`
  (`crates/node/src/state/staking.rs:185-194`), prefix `k:` (`:32`), epochs
  of `epoch_blocks` (`:50`). The bond secures DAG-BFT ordering and
  finality (ADR-027/028) and nothing else.
- **Slashing is evidence-based and verified by Maya2C itself.** An
  `Equivocation` is checked with `MlDsaAuthenticator::observer(keys, chain_tag)`
  and its author must be in the committee (`state/staking_exec.rs:256-268`).
  The chain decides what counts as a fault from two signed messages it can
  verify. No external party tells it.
- **Exit windows are derived.** ADR-096 ties `MIN_UPGRADE_LEAD` to the
  vault delay plus unlock plus unbonding, so a holder can always leave
  before a rule changes. Anything that lengthens the time stake is at risk
  (a second unbonding period for an exported duty) changes that derivation.
- **No oracle by default** (invariant 11). A slashing condition evaluated
  off-chain ("the AVS says the operator misbehaved") is an oracle, and the
  chain's trusted party, introduced by whoever wrote it down.
- **Governance cannot make governance unsafe** (invariant 12). A rule
  that lets an external contract remove validator stake can change who is
  in the committee, which is a governance-safety question.
- **ADR-016** lists what mainnet v1 is; stake export is not on either
  table. Memory `owner-directive-permanent-first-2026-11-07`: only
  genesis-permanent parts before 12-03. None of this is genesis-permanent.

**Findings that contradict the brief.**

1. The parity row says "Cosmos ICS now permissionless". The producer of
   ICS is removing it: the Cosmos Hub forum carries a two-phase proposal to
   disable new consumer chains and then delete the ICS module (prior-art
   file, forum thread 16682), calling it "heavy technical debt"; Gaia v28,
   which removes ICS, was on the public testnet in August 2026. ICS 2.0
   (opt-in partial set) exists and Stride and Neutron used it; adoption is
   described as "a burden" by validators. The trend the row cites is
   reversing at its source.
2. Babylon's slashing (extractable one-time signatures, Schnorr key
   leakage on double-signing) has no equivalent for ML-DSA or SLH-DSA in
   this tree, and none was searched for. A Maya2C export cannot slash by
   key extraction; it must slash by evidence, which is the ADR-028 model
   and is fine, but it means the fault must be two Maya2C-verifiable
   signed messages, not an external chain's verdict.
3. The row cites invariant 32 (vault windows). A vault that is also a
   validator already exists; export changes nothing about 32 unless a
   slash can take funds out of a vault faster than its window allows,
   which ADR-028 slashing already does. 32 is touched only in that a
   second slashing *source* widens an existing exception.

## Options

### A. None: stake secures Maya2C and nothing else

No export, no restaking, no attestation of stake to outsiders beyond what
a `StateLayer::Staking` proof against a finalized header already gives
(`state/proof.rs:311` for layer proofs). Anyone who wants to prove a
Maya2C validator is bonded can do so today with a light client. No
slashing by anyone but Maya2C's own rules.

### B. Internal opt-in duties (Cosmos "partial set" shape, inside Maya2C)

Validators opt in (`k:opt:` ‖ validator_id ‖ duty_id) to extra duties
defined **by Maya2C ADRs** and verified by Maya2C rules: blob sampling
attestations (ADR-123), storage-market auditing (ADR-133), appchain
checkpoint signing (ADR-134). A duty's fault is two conflicting messages
signed with the validator's Maya2C key, or a missed attestation count,
both evaluated on chain. Slash per duty capped at `duty_slash_cap_bps`
(governed, invariant 17) of the bond, and the sum of all duty slashes in
one epoch capped at `restake_total_cap_bps`, so a validator cannot be
fully slashed for anything but Maya2C double-signing. The duty pays from
its own escrow (ADR-133 payments, ADR-134 fees), never from issuance.

### C. External restaking (EigenLayer / Symbiotic shape)

A `RestakeCommit` records that `amount` of a validator's bond backs
external service `svc` whose slashing conditions are evaluated by a
contract or a registered "slasher" address. The service submits a
`SlashRequest`; the chain executes it after a veto window in which a
governed resolver set can cancel (Symbiotic's VetoSlasher). The chain
cannot verify the fault; it trusts the slasher plus the resolvers. Stake
committed this way is subject to the longer of Maya2C's and the service's
unbonding.

### D. Stake pointer without slashing (Symbiotic "no slasher" vault)

A validator signs an attestation "I hold X bonded until height H" and the
chain stores it under `k:att:`; outsiders read it. No slashing, so no
security is actually exported — it is a reputation signal. Equivalent to
A plus a convenience record; the light-client proof in A already gives
the same thing without a new prefix.

## Recommendation

**A at launch and through the first year; B as the shape if any export
is ever wanted; C refused.** Reasons:

1. C introduces the chain's trusted party (the slasher) against invariant
   11, lets an outside contract change the committee against invariant
   12, couples two unbonding clocks against ADR-096, and imports the
   cascade risk every surveyed source names (one service's penalty
   reduces the collateral left for the others — Symbiotic's own docs say
   so). EigenLayer's slashing went live in April 2026 (sources disagree
   on the year; prior-art file) with roughly 1,500 operators holding most
   of the stake per one weak source; the model is not proven at the scale
   a new chain would be betting its validator set on.
2. B keeps every fault verifiable by the chain, caps the exposure in
   governed basis points, and funds duties from escrow, which is what
   ADR-044's "nothing is minted" already requires. It is what ADR-133 and
   ADR-134 would need if adopted, and nothing else needs it.
3. D is A with extra state. Not worth a prefix.
4. ADR-016's own rule: a deferred module activates at a written height
   after its own evidence. There is no evidence anybody wants Maya2C stake
   to secure something else; memory `owner-directive-no-third-party-2026-10-28`
   says do not raise outside parties before mainnet.

## Consequences per option

| | A none | B internal duties | C external restaking | D pointer |
|---|---|---|---|---|
| New state | none | `k:opt:`, per-duty slash caps in `Params` | `k:rs:`, `k:slash:req:`, resolver set | `k:att:` |
| Who decides a fault | Maya2C rules | Maya2C rules | the service's slasher + resolvers | nobody |
| Max loss per validator per epoch | `double_sign_slash_bps` | + `restake_total_cap_bps` | up to the committed amount per service, summed | none |
| Exit window (ADR-096) | unchanged | unchanged (duties end with the bond) | max of two unbonding periods; derivation changes | unchanged |
| Invariant 11 | holds | holds | **broken** (slasher is an oracle) | holds |
| Committee safety (12) | n/a | slash cap keeps a validator above `min_self_bond` unless Maya2C-slashed | an external slash can empty a seat | n/a |
| ADR-016 scope | inside | outside; needs its own evidence row | outside | outside |

## Invariants touched

- **11**: C adds a trusted party; A, B, D do not.
- **12**: B's caps must be inside the governance range checks
  (`crates/governance/src/limits.rs`), so a vote cannot set
  `restake_total_cap_bps` to 10,000 and let a duty empty the committee.
- **17**: every cap and window is a governed value read from state.
- **25**: B and C add `k:` sub-prefixes inside `StateLayer::Staking`
  (already in `RECORD_LAYERS`); undo coverage for opt-in and slash records.
- **28**: a duty breaker (`Module::Duties`) halts duty slashing and duty
  payouts, never the staking payout and never a transfer.
- **32**: a duty slash on a vault-validator is bounded by the same cap; it
  is a second path that bypasses the window and must be named in 32's
  text if B is adopted.
- **ADR-096**: C changes the exit derivation; B does not.

## 256-bit math

Slash = `bond × bps / 10_000` in checked `U256`; `bps ≤ 10_000 < 2¹⁴`,
`bond < 2²⁵⁶`, so the product can overflow in principle. Existing slashing
code already does this multiplication; whichever bound it uses (the
`mul_div` timing decision in memory `u256-ceiling-no-512`) applies
unchanged. No new arithmetic shape.

## Test plan if adopted (B)

Red tests, `crates/node/tests/duty_tests.rs`:

- `before_the_height_opt_in_is_refused_and_the_root_matches_master`
- `a_duty_slash_never_exceeds_duty_slash_cap_bps_of_the_bond`
- `the_sum_of_duty_slashes_in_an_epoch_never_exceeds_restake_total_cap_bps`
- `a_duty_slash_cannot_take_a_validator_below_min_self_bond_unless_it_double_signed`
- `a_duty_fault_needs_two_maya2c_verifiable_signatures_and_refuses_a_bare_claim`
- `opting_out_ends_at_the_bond_unbonding_and_adds_no_second_window` (ADR-096 pin)
- `a_tripped_duties_breaker_halts_duty_slashing_and_not_the_staking_payout` (28)
- `a_reorg_restores_opt_in_records_and_slash_records` (25)
- `governance_cannot_set_restake_total_cap_bps_outside_the_range` (12)
- `exploit_replays::a_colluding_duty_cannot_remove_a_committee_seat` — prints the real maximum loss.

Spec: staking section gains "duties"; vectors DUTY-1..DUTY-3.

## Activation

`FeatureSet::STAKE_DUTIES = 1 << 38` (reserved in `adr-claims.txt`; unused
under A), compiled height `u64::MAX`, scheduled under ADR-082 after ADR-096
is live, and after whichever of ADR-133/134 needs it.

## Decision needed from Eric

1. A (no export) or B (internal opt-in duties with capped slashing)? C is
   presented for completeness; the recommendation is to refuse it.
2. If B: the two caps at activation, `duty_slash_cap_bps` and
   `restake_total_cap_bps` (the ADR proposes no number; nothing is measured).
3. If B: may a duty slash bypass a vault's window (invariant 32), as
   double-sign slashing already does, or must duty losses respect the window?
4. Is securing an outside chain with Maya2C stake ever in scope (ADR-016
   amendment), or is the answer "appchains use their own keys" (ADR-134
   option B)?
