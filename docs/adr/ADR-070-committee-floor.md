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

## Amendment 2026-11-14: pinned on the node; open at mainnet bonds

Founder-brief audit question 17 (Eric 2026-11-14 chose option A, "merge the fix before
the freeze") found the S-033 row still `open` with a `pending` fix. The floor had merged
as #246 (46a97a61, 2026-10-30) and is in the frozen node commit bb4e0940.

**The floor does not hold at mainnet parameters.** The downtime slash runs before the
count (`epoch.rs`, "Slash first, then count"), `GenesisStaking::params` keeps the
DEVNET `downtime_slash_bps = 100`, and `xtask genesis-check` makes every mainnet bond
exactly `min_self_bond` (Eric 2026-11-07, the tiny bond). A down validator therefore
falls to 99% of the minimum, is ineligible, and is jailed whatever the floor says ("the
ineligible are always jailed"). The committee can still be jailed below four seats, and
one more failure is attack 7's permanent halt. S-033 stays `open` until a re-freeze fix
lands. That fix changes a consensus rule or a genesis decision, so it is Eric's choice
(Standing Order 9). The options, none implemented here:

- (a) a downtime slash does not make a floor-kept validator ineligible: count the floor
  before the bond-minimum check, and keep it eligible at the next committee selection;
- (b) cap the downtime slash at `self_bond - min_self_bond`, which at mainnet bonds means
  no downtime slash at all;
- (c) genesis-check requires a margin above the minimum, which reverses the tiny-bond
  decision.

Two in-process node tests with the shape of attack 7 (`crates/node/tests/bft_follower_tests.rs`;
they are not the attacknet harness, and no post-#246 attacknet attack-7 run is cited) pin
this. One validator of four is down for the whole first epoch, is judged down (its bond
is exactly 1% lower), and a second then fails. The chain halts, and it should resume
when the first restarts by certificate replay:

- `a_validator_kept_by_the_committee_floor_ends_the_halt_when_it_restarts`, bonds ten
  times the minimum: passes, and fails with `MIN_COMMITTEE` set to 0, both at the seat
  count and, with those asserts removed, at the node-level restart;
- `at_mainnet_bonds_the_committee_floor_ends_the_halt_when_it_restarts`, bonds equal to
  the minimum: fails today, the boundary seats three. It is `#[ignore]`d until the fix.

Commands and output: `reports/sessions/2026-11-14-s033-floor.md`.

Beyond any fix, the original "still open" line remains: if more than f seated
validators are gone for good, no quorum exists that could sign a change of committee.
That follows from the quorum rule (2f + 1 of n = 3f + 1, ADR-027; W − f by weight,
ADR-039/ADR-040), not from a test here. Recovery stays the social re-genesis in
`docs/runbooks/chain-halt.md` (`docs/audit/KNOWN_ISSUES.md` #18, "recovery open").
