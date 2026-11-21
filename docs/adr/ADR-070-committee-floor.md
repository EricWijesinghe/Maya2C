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
