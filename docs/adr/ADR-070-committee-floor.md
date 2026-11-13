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
ties by id, so every node chooses the same ones.

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

## Amendment 2026-11-14: pinned on the node; the residual is a BFT limit

Founder-brief audit question 17 (Eric 2026-11-14 chose option A, "merge the fix before
the freeze") found the S-033 row still `open` with a `pending` fix. The fix had already
merged as #246 (e28f1f25, 2026-10-30) and is in the frozen node commit bb4e0940, so
option A needs no re-freeze. The row was stale.

The floor is now also pinned on the node, not only in the staking crate:
`a_validator_kept_by_the_committee_floor_ends_the_halt_when_it_restarts`
(`crates/node/tests/bft_follower_tests.rs`) runs attack 7 on four validators. One is down
for the whole first epoch, is slashed and stays seated, and a second then fails. The chain
halts, and it resumes when the floor-kept validator restarts by certificate replay. With
`MIN_COMMITTEE` set to 0 the test fails, because the boundary seats three.

The "still open" line above is not a missing fix. If more than f seated validators are
gone for good, no quorum exists that could sign a change of committee. Any rule that let
fewer sign one would let a minority fork the chain. Recovery stays the social re-genesis
in `docs/runbooks/chain-halt.md` (KNOWN_ISSUES #18, "recovery open").
