# ADR-070: downtime jailing keeps at least four eligible validators

- Status: Accepted (2026-10-29, Claude under Eric's delegation: "fix, debug, update,
  upgrade … fully authorized you to decide"). Self-run pre-audit S-033.
- Scope: consensus (staking epoch boundary), pre-genesis. Takes effect at the re-genesis.

## Context

The self-run attacknet on 086892ac (`reports/attacknet/2026-10-29-2.md`) halted for good at
height 240. Earlier attacks had left validators down for an epoch, so the boundary jailed
them, and the stolen-key attack retired another. The committee shrank to a few seats, and
attack 7 took one more down, leaving no quorum. Jailed validators return only at an epoch
boundary, and a halted chain never reaches one, so the halt could not end without a new
genesis (`docs/runbooks/chain-halt.md`).

## Decision

At an epoch boundary every validator below the downtime threshold is still slashed and
paid nothing, but at most `eligible − MIN_COMMITTEE` of them are jailed, with
`MIN_COMMITTEE = 4`, the smallest committee that survives one fault (n = 3f + 1, f = 1).
The worst participation is jailed first (authored ÷ expected, compared in integers),
ties by id, so every node chooses the same ones. (Amended 2026-11-21, below: the floor is
counted before the slash, and a kept validator is not slashed below `min_self_bond`.)

## Consequences

- A down validator the floor keeps stays seated. Its node can restart and vote at once,
  so a halt caused by downtime ends when operators bring nodes back. Before, it could not
  end at all.
- A floor-kept validator that stays down still costs the committee a seat. The floor
  restores the way back; it does not create a quorum that is not there.
- Still open: a halt that no restart can end, because more than f seats are gone for good.
  The way out stays the social re-genesis in the runbook, so mainnet gates 8 and 10 stay
  "running" until an attacknet passes on this rule.
- Consensus-visible: the jailed set and therefore the committee and the state root change.
  Re-genesis only.

## Amendment 2026-11-21: the floor holds at mainnet bonds

Founder-brief audit question 17 (Eric 2026-11-14 chose option A, "merge the fix before
the freeze", and "I approve everything, proceed") found the S-033 row still `open`. The
floor had merged as #246 (46a97a61, 2026-10-30) and is in the frozen node commit bb4e0940.

**The merged floor did not hold at mainnet parameters.** The downtime slash ran before the
count ("Slash first, then count"), `GenesisStaking::params` keeps the DEVNET
`downtime_slash_bps = 100`, and `xtask genesis-check` makes every mainnet bond exactly
`min_self_bond` (Eric 2026-11-07, the tiny bond). A down validator fell to 99% of the
minimum, was ineligible, and was jailed whatever the floor said. On seven seats, one down
validator per epoch went 7, 6, 5, 4, 3, and one more failure is attack 7's permanent halt.

**Decision.** The floor is counted before the slash, over the down validators that are
eligible now. It keeps `MIN_COMMITTEE − (eligible validators that are not down)` of them,
the best participation first, ties by id (the same order that jails the worst first). A
kept validator is unpaid, not jailed, and slashed only when the slash leaves its self bond
at or above `min_self_bond`. Every other down validator is slashed and jailed, as before;
one that was already ineligible is always jailed, so a top-up cannot skip the term.

Options weighed (the 2026-11-14 review):

- (a) keep a floor-kept validator eligible after the slash. Chosen, in the form above: the
  smallest change to the rule; unlike (b) and (c) it leaves the jailed validators' slash
  and the tiny-bond genesis untouched;
- (b) cap every downtime slash at `self_bond − min_self_bond`. Rejected: at mainnet bonds it
  removes the downtime slash for everyone, not only while the floor binds;
- (c) require a bond margin above the minimum in genesis-check. Rejected: it reverses the
  tiny-bond decision, and a later top-down of bonds would reopen the hole.

Consequences: at mainnet bonds a validator kept by the floor loses its epoch reward but no
bond, so downtime while the committee is at four seats costs less than downtime above it.
Which validator is kept can change: with thin bonds among the down ones, the best
participant is kept even if its bond is thin, where before the thin bonds were jailed and
the worst offender kept. Consensus-visible (the slashed amounts, the jailed set, the
committee, the state root): re-genesis only, so this is re-freeze work.

Tests:

- `at_mainnet_bonds_downtime_never_shrinks_the_committee_below_four_seats`
  (`crates/staking/tests/scenario.rs`): seven validators at exactly the minimum, one more
  down each epoch. Failed before the change at epoch 4 with three seats; passes after.
- `the_committee_floor_is_counted_after_the_slash`: updated to the new choice of kept
  validator (thin bond 6 kept unslashed, the worst offender and thin bond 5 jailed).
- Two in-process node tests with the shape of attack 7 (`crates/node/tests/bft_follower_tests.rs`;
  not the attacknet harness, and no post-#246 attacknet attack-7 run is cited). One
  validator of four is down for the whole first epoch, a second then fails, the chain
  halts, and it resumes when the first restarts by certificate replay:
  `a_validator_kept_by_the_committee_floor_ends_the_halt_when_it_restarts` (bonds ten times
  the minimum; fails at the post-restart height with `MIN_COMMITTEE` set to 0 and the seat
  asserts removed) and `at_mainnet_bonds_the_committee_floor_ends_the_halt_when_it_restarts`
  (bonds equal to the minimum; failed before this change, three seats, passes after).

Commands and output: `reports/sessions/2026-11-14-s033-floor.md`.

Beyond any fix, the original "still open" line remains: if more than f seated
validators are gone for good, no quorum exists that could sign a change of committee.
That follows from the quorum rule (2f + 1 of n = 3f + 1, ADR-027; W − f by weight,
ADR-039/ADR-040), not from a test here. Recovery stays the social re-genesis in
`docs/runbooks/chain-halt.md` (`docs/audit/KNOWN_ISSUES.md` #18, "recovery open").

## Amendment 2026-11-24: Eric chooses option A; no exit breaks the floor (CON-07)

**S-033 decided (Eric 2026-11-24).** Option A of decision brief 1, the rule built in the
amendment of 2026-11-21 above, is the rule: the floor is counted before the slash, and a
validator the floor keeps stays eligible. Option B (a genesis bond margin, the 2026-11-22
draft) is not taken, so every mainnet seat still bonds exactly the minimum. STK-11 in
`spec/06-staking.md` states the rule. The guarantee: **punishment alone never shrinks the
committee below four seats.**

**CON-07 (Eric 2026-11-24, recommendation B, pros and cons seen).** The downtime path was
the only one floored. Two others could still take the set below `MIN_COMMITTEE`:

- **A whole-bond unbond waits for a replacement.** `Staking::unbond` of the whole self bond,
  while the validator is eligible and at most four are, is accepted but deferred: the
  validator joins `Staking::exits`, stays bonded, seated, slashable and paid, and its bond
  starts the normal unbonding delay at the first epoch boundary where four others are
  eligible (`process_exits`, before the epoch advances, so the release is
  `unbonding_epochs` after that boundary). Nothing is refused, so no stake is trapped on a
  small network; the exit only waits. A partial unbond (remainder at or above the minimum)
  never waits. A second unbond while waiting is refused (`StakeError::Leaving`). STK-13.
- **A tombstone keeps its seat until a replacement exists.** The slash and the tombstone are
  immediate and permanent. Only the removal from the committee is deferred: while fewer than
  four validators are eligible, `next_committee` refills from the ending committee's
  tombstones, **at most one** (`MAX_HELD_TOMBSTONES = (MIN_COMMITTEE − 1) / 3`). A held
  tombstone earns nothing. STK-14.
- **What the hold trades (review of PR #704, 2026-11-24).** Holding even one tombstone puts
  liveness before safety. The quorum is `W − f` (`crates/dag-bft/src/vertex.rs`). Without
  the hold the committee has three seats, `f = 0`, quorum 3: one further Byzantine member
  can halt it but cannot make it unsafe. With the hold it has four, `f = 1`, quorum 3: the
  proven equivocator has spent the fault budget, so one further Byzantine or compromised key
  can sign a conflicting certificate with it. The liveness gained also depends on the
  equivocator still signing. Its slash is idempotent once tombstoned and its bond is locked,
  so further equivocation costs it nothing more. Mid-epoch, `active` keeps every tombstone
  while fewer than four are eligible (`slash_double_sign`), so it can name two until the
  boundary; only one is carried into the next committee. **Raised with Eric:** keep the
  hold as decided, or hold the seat at zero weight (it then counts for the seat floor but
  toward no quorum, which is the three-seat safety with the four-seat bookkeeping).

Waiting exits are state: the staking record is version 4, with an `exits` section, only
while one waits; a record with none keeps its version-3 bytes, so no existing state root
moves. Consensus-visible (committee, bonds, unbonding queue, state root): re-freeze work.

Tests (red: with the deferral and the hold disabled, `cargo test -p maya-staking --test
committee_exits` gave `FAILED. 2 passed; 4 failed`):

- `crates/staking/tests/committee_exits.rs`: `a_whole_bond_exit_at_the_floor_waits_for_a_replacement`,
  `an_exit_above_the_floor_leaves_at_once`, `a_partial_unbond_at_the_floor_is_not_delayed`,
  `a_tombstone_at_the_floor_is_slashed_now_and_keeps_its_seat_until_a_replacement`,
  `the_floor_never_holds_more_tombstones_than_the_one_fault_it_survives`,
  `a_waiting_exit_that_is_then_tombstoned_is_dropped_from_the_queue`.
- `crates/node/src/state/staking.rs`: `a_waiting_exit_round_trips_and_only_its_presence_changes_the_version`.
- `crates/staking/tests/offender_unbond.rs` now seats four bystanders, so its lone offender's
  exit is above the floor and starts at once, as those tests assume.

Not covered: the floor counts validators, not stake. On a stake-weighted chain a held
tombstone votes with what is left of its stake after the slash. (Superseded by the
third-round amendment below: it now votes with zero.)

## Amendment 2026-11-24 (third round): a held tombstone votes with zero weight

Eric 2026-11-24, third round, decision 5, answering the question raised above: a
tombstoned validator the floor holds in its seat has zero consensus weight. The seat
counts for the floor (four members, a leader slot); its weight counts for no quorum.

**The rule (STK-22).** `stake_weights` gives a tombstoned committee member weight 0,
before the operator cap. DAG-BFT's `Committee::weighted` keeps a zero weight as zero (it
used to raise it to one) and refuses a non-empty committee whose weights are all zero. If
every member of a committee would have weight 0, the members keep their stake weights:
ADR-028 keeps a chain with no eligible validator running for governance to fix, and a
zero total would make any set a quorum.

**The guarantee.** With four seats and one held tombstone the quorum is the three voters'
`W − f`, so the trade recorded in the amendment above is gone on a stake-weighted chain:
the floor keeps four seats and the safety of three. The proven equivocator's signature
never completes a certificate, an attestation checkpoint or a catch-up quorum (all three
build `Committee::weighted` from the same frozen weights). Weights are frozen at the
boundary, so in the epoch of the offence the tombstone keeps the weight it was frozen
with; it has weight 0 from the first epoch it is held. It keeps its leader slots: an
anchor it proposes commits only on voters' weight, and an anchor it withholds costs one
slot, as any absent leader does.

**With the dual quorum (third round, decision 3).** The count rule is counted over
voters, the members with positive weight (`Committee::voters`), and only voters'
signatures count toward it (`Committee::voters_in`). A held tombstone is not a voter:
counting its seat would let a proven equivocator's key stand in for an honest member in
the count rule, the kind of cheap head the count rule exists to stop. Four seats with
one held: the count rule needs `3 − ⌊(3 − 1)/3⌋ = 3` voters, which the weight rule
already requires.

**Not covered.** A one-seat-one-vote chain (`stake_weighted = false`, no weights record)
still counts the held seat as one head: it has no weights to zero. `genesis-check`
refuses a value-bearing genesis that is not stake-weighted, so this is a devnet and
test-chain gap only.

Tests: `a_zero_weight_seat_is_seated_but_never_counted`, `a_committee_with_no_weight_is_refused`
(`crates/dag-bft/src/vertex.rs`); `a_tombstone_held_by_the_floor_votes_with_zero_weight`
(`crates/node/src/state/staking.rs`); `a_zero_weight_seat_is_not_an_operator_the_cap_counts`
(`crates/node/src/state/weight_cap.rs`: counted as an operator, the zero seat dropped the
CON-06 water-filling level to 0 and flattened every voter to weight 1, found by the
staking test above); `a_held_tombstone_votes_with_zero_weight_and_the_chain_runs_on`
(`crates/node/tests/bft_staking_tests.rs`). Red before the change: the dag-bft test did
not compile (no `voters`), and with `[5, 5, 5, 0]` read as `[5, 5, 5, 1]`, as
`Committee::weighted` used to, the set `{0, 1, 3}` weighs 11 of 16, a quorum.
