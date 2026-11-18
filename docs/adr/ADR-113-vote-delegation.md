# ADR-113: Vote delegation and conviction on locked stake

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-27, [prior-art/vote-delegation.md](../prior-art/vote-delegation.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md), [ADR-111](ADR-111-treasury-spend-and-grants.md), `docs/governance.md`; invariants 7, 12, 13, 17, 25, 28; Standing Order 4; Eric's 256-bit rule (2026-11-01)

## Context

A vote's weight is the voter's own locked stake plus decayed work credit,
read at cast time (`voting_power`, `crates/node/src/state/governance_exec.rs:142-150`;
`cast_vote` `:485-528`). One ballot per `(proposal, voter)` is enforced by
the key `g:vote:<proposal><voter>` existing (`crates/node/src/governance/mod.rs:56-61`),
and the voter's lock must outlive the proposal's execution
(`lock_required_until`, `:507-516`): "the cheapest way to decide something
is to acquire weight, vote, and be gone". A `StakeLock` is extended, never
shortened (`crates/node/src/core/governance_payload.rs:127-137`). The quorum
denominator is locked stake plus work credit of everyone
(`settle_governance` `:575-590`); `crates/governance/src/tally.rs:15-24`
explains why turnout may exceed the denominator. Floors are compiled
(`crates/governance/src/limits.rs:33-74`): `MIN_VOTING_BLOCKS 480`,
`MAX_VOTING_BLOCKS 100_000`, `MIN_TIMELOCK_BLOCKS 960`,
`MAX_TIMELOCK_BLOCKS 200_000`, `EXECUTION_GRACE_BLOCKS 100_000`.

There is no delegation: a holder who does not vote is turnout nobody
represents. Cardano's DReps take delegated stake one hop deep; Polkadot's
conviction multiplies a vote by a longer lock (prior-art file). The parity
matrix notes "stake-weighted only until PoP": representative voting by
persons needs proof of personhood (`features.toml` personhood RESEARCH),
and this ADR is stake-weighted.

**Finding that contradicts the brief (reported, not worked around).**
`crates/governance/src/tally.rs:40` and `:265` take threshold products
through the 512-bit `maya_amount::Wide`. Eric's rule of 2026-11-01 is 256
bits with no 512-bit intermediates; that site predates the rule and is on
the list awaiting a timing decision. This ADR adds no new product to the
tally: delegated weight is an addition.

## Decision

**A holder delegates its locked stake to one delegate, one hop, with a
conviction it chooses. The delegate's ballot carries its own weight plus
the delegated total, kept as a running sum. Delegated stake is withdrawable
only after an undelegation lead long enough for every proposal the delegate
could have voted on to have executed or expired.**

### State (invariant 25), all under the committed `g:` prefix

- `g:dlg:` ‖ `delegator` → `Delegation { delegate: Address, conviction: u8,
  weight: U256, undelegate_at: Option<u64> }`, where `weight = locked <<
  conviction` at the time of delegation (updated on `LockStake`).
- `g:dpw:` ‖ `delegate` → `U256`, the sum of `weight` over every delegation
  to it. Maintained by `checked_add`/`checked_sub`; an overflow refuses the
  delegation (the sum of locks cannot exceed supply, so the check is a
  tripwire, not a path).
- `g:udl:` ‖ `undelegate_at(8 BE)` ‖ `delegator` → empty: the due index, in
  key order, scanned from the start by `settle_governance` each block.

### Operations

| TxKind | Rule |
|---|---|
| `Delegate { to, conviction }` | Sender has a non-zero lock and no delegation; `to != sender`; `to` has no delegation of its own (**one hop**: a delegate that delegates would make `g:dpw` a graph walk); `conviction <= MAX_CONVICTION`. Writes `g:dlg:`, adds `weight` to `g:dpw:to`. |
| `Undelegate` | Sender has a delegation with `undelegate_at = None`. Sets `undelegate_at = height + UNDELEGATE_LEAD_BLOCKS + (CONVICTION_PERIOD_BLOCKS << conviction)` and writes `g:udl:`. Nothing else moves. Calling it again is a **no-op** (invariant 7). |
| `CastVote` by a delegator | Refused while `g:dlg:` exists for the sender: its weight is in its delegate's hands until the undelegation completes. Work credit is never delegated and is lost to a delegator who chooses delegation; this is stated in the wallet. |
| `CastVote` by a delegate | `weight = own_lock + own_work + g:dpw:self`, with the existing lock check on the delegate's own lock. One ballot per `(proposal, delegate)` as today. |
| `LockStake` by a delegator | Allowed; the delta `<< conviction` is added to `g:dlg.weight` and `g:dpw:delegate`. |
| `UnlockStake` by a delegator | Refused while `g:dlg:` exists. The lock leaves only after the undelegation completes. |

### Completion (the consensus step)

In `settle_governance` (`governance_exec.rs:575`), before proposals are
settled, scan `g:udl:` from the start while `undelegate_at <= height`, at
most `MAX_UNDELEGATIONS_PER_BLOCK = 256`: subtract `weight` from
`g:dpw:delegate`, delete `g:dlg:` and `g:udl:`. Entries beyond the 256th
stay due and complete first in the next block (first-fit by height, the
shape of invariant 33 and ADR-109).

### Why the lead is what it is

`UNDELEGATE_LEAD_BLOCKS = MAX_VOTING_BLOCKS + MAX_TIMELOCK_BLOCKS +
EXECUTION_GRACE_BLOCKS = 400_000`, compiled in `limits.rs`. A delegate who
voted with this stake on any proposal open at the moment of `Undelegate`
sees that proposal execute or expire before the stake can leave, which is
the same guarantee `lock_required_until` gives a direct voter, derived
from the same compiled bounds (invariant 12). A governed lead could be
voted down to zero by the stake it protects against.

### Conviction

`MAX_CONVICTION = 6` and `CONVICTION_PERIOD_BLOCKS = 2_592_000` (30 days at
one block per second) are compiled. A delegation at conviction `c` counts
`locked << c` (at most `x64`) and adds `CONVICTION_PERIOD_BLOCKS << c` to the
undelegation lead, up to about five years at `c = 6`. A **direct** voter
may also pass `conviction` in `Ballot { proposal, choice, conviction }`: its
lock is extended at cast time to `lock_required_until + (CONVICTION_PERIOD_BLOCKS << c)`
(extension only, never shortening, `governance_payload.rs:131-136`) and its
own locked term is shifted; work credit is never multiplied. `locked << c`
is refused if `locked >= 2^250`, bound written at the check, so no shift
overflows 256 bits; no multiplication, no 512-bit intermediate.

The quorum denominator (`:580-586`) is unchanged and unmultiplied, as
Polkadot's support measure is; conviction moves only the approval ratio.
Turnout already may exceed the denominator (`tally.rs:21-24`); conviction
makes quorum easier, never harder, and the binding threshold is approval.

## Alternatives considered

- **Snapshot tally at close (Cardano).** Counts delegation as of an epoch
  snapshot, which would replace cast-time accumulation and the
  `g:vote:` key rule; a different governance engine. Rejected for v4.
- **Delegator override (vote directly, subtracting from the delegate).**
  Needs the delegate's ballot found and reduced per delegator, and a
  delegator could override after seeing how the delegate voted, then
  undelegate. Rejected; delegation is exclusive until completed.
- **Multi-hop delegation (liquid democracy).** A cycle check and a graph
  walk at cast time. Rejected; one hop, as DReps.
- **Linear conviction multipliers.** Polkadot's doubling is chosen so that
  the lead and the weight scale together; a shift is also cheaper than a
  multiplication.
- **Representative voting by persons.** Needs proof of personhood; not in
  this ADR.

## Consequences

- Holders who do not follow governance can be represented; delegates
  accumulate real, locked weight.
- Three prefixes under `g:`, two new kinds under `Module::Governance`
  (invariant 28), one new field on `Ballot` (trailing, length-defaulted to
  conviction 0), one bounded step in `settle_governance`.
- A delegator's work credit stops counting for anyone; the wallet says so.
- Delegated stake locks for at least `400_000` blocks after `Undelegate`;
  wallets show the completion height.

## Invariants touched

- **12**: the lead and conviction constants are compiled and derived from
  the existing floors; no `ParameterKey`.
- **13**: a delegation is an address and a number.
- **17**: nothing new is a `const` read at a call site; the constants here
  are floors, which is what invariant 12 says a `const` is for.
- **25**: `g:dlg:`, `g:dpw:`, `g:udl:` under `g:`; undo covered.
- **28**: `Module::Governance`; key-order scan, bounded.
- **7**: a second `Undelegate` is a no-op.
- Standing Order 4: heights only; `BTreeMap`/key order; no floats.

## Activation

`FeatureSet::VOTE_DELEGATION = 1 << 21` (claimed in `adr-claims.txt`),
compiled `VOTE_DELEGATION_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**. Before activation
`Delegate`, `Undelegate` and a non-zero `conviction` are refused on
admission and on apply, and the completion scan is skipped.

## Test plan

Red tests, `crates/node/tests/vote_delegation_tests.rs`:

- `a_delegates_ballot_carries_its_delegators_locked_stake`
- `a_delegator_cannot_vote_or_unlock_while_delegated`
- `a_delegate_that_delegates_is_refused_one_hop_only`
- `an_undelegation_completes_at_the_lead_and_not_one_block_earlier`
- `a_delegate_cannot_be_voted_through_twice_by_the_same_stake` (vote, undelegate, wait, vote directly: the second cast lands after the proposal closed)
- `conviction_shifts_the_locked_term_and_never_work_credit`
- `a_lock_at_two_to_the_250_is_refused_before_shifting`
- `the_257th_due_undelegation_completes_in_the_next_block_first`
- `a_reorg_restores_dpw_dlg_and_the_due_index`

Property test in `crates/governance/tests`: for random delegate, lock,
undelegate and complete sequences, `g:dpw:d == sum of weights of live
delegations to d`. Vectors (`spec/03-state.md` GV-5..GV-8): record bytes;
a delegation at conviction 3; its completion height; a ballot with
conviction.

## Open questions for Eric (economics and pricing only)

1. Delegate compensation: none (as written), or a share of
   `GovernanceProposalDeposit` or of ADR-111's treasury for delegates
   whose ballots meet a turnout floor? Cardano's DRep incentives are
   unresolved too.
2. A deposit to register as a delegate, to make a thousand one-block
   delegates cost something?
3. `CONVICTION_PERIOD_BLOCKS` (30 days as written) and `MAX_CONVICTION` (6,
   so x64 at about five years).
