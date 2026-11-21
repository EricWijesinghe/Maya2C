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

## Amendment 2026-11-22: option A + B, pending Eric

**Status: proposed, pending Eric.** Owner decision brief 1 (the independent AI audit of
2026-11-19, `14-decision-briefs.md` item 1; an AI review, not an external audit) asks Eric to
choose between four options for the same S-033 gap. Draft PR "[needs-eric] fix(staking): S-033
committee floor at mainnet bonds (brief 1, option A+B)" builds the brief's recommendation; it is
stacked on #524 and merges only with Eric's choice.

| Brief option | What it is | Where |
|---|---|---|
| A | the floor is counted before the slash; a floor-kept validator stays eligible | already built by #524 (amendment 2026-11-21 above); this amendment keeps it unchanged |
| B | genesis bonds carry a margin above `min_self_bond` | built here, in `cargo xtask genesis-check` and `genesis-ceremony` |
| C | a validator the floor keeps is not slashed at all | not built |
| D | accept and document | not built |

**B as built.** Every mainnet seat bonds exactly the genesis bond, `min_self_bond` plus
`BOND_MARGIN_BPS` = 10.6%, rounded down (1,106 coins at the 1,000-coin floor). One amount for
every seat keeps what the tiny-bond decision (Eric 2026-11-07) was for: the bonds are the
disclosed seats, the 20% operator cap stays a seat count, and genesis mints no more than
seven small bonds. The margin is the least, in tenths of a percent, that leaves a bond at or
above the minimum after `MARGIN_OUTAGES` = 10 downtime slashes at the node's compiled 1%
(1,106 coins leave 1,000.25 after ten slashes; at 10.5%, 1,105 coins leave 999.34). A genesis file cannot change that rate:
`StakingGenesis::params` takes `downtime_slash_bps` from `Params::DEVNET`, and the xtask test
reads it from there. The node itself is unchanged and does not check the margin, so B is
launch policy, not a consensus rule (`spec/06-staking.md`).

**Why A and B together.** A alone holds the floor, but at bonds of exactly the minimum a kept
validator can never be slashed (any slash would unseat it), so downtime while the committee
is at four seats costs only the epoch reward — the economics note in #524. With B the kept
validator's slash leaves it above the minimum, so it is slashed like any other down validator
for its first ten outages: A keeps the seat, B restores the penalty. B also lets a validator
jailed above the floor return from its term without a top-up, which a bond of exactly the
minimum cannot (990 < 1,000 after one outage).

**What B does not cover.** A validator registered after genesis at exactly `min_self_bond`
brings back the thin-bond case for itself; A still keeps the floor, unslashed. Ten outages is
a margin, not a bound: the eleventh slash takes a genesis bond below the minimum, and A then
keeps it seated unslashed while the floor binds.

**What changes at genesis if Eric takes A + B.** The ceremony's `validators.txt` names 1,106
coins per seat (`1106000000000000000000000000` base units) with `--min-self-bond` still 1,000
coins; genesis carries 7,742 coins of bonds instead of 7,000. `genesis-check` refuses a bond
of exactly the minimum. `deploy/genesis/mainnet/README.md`,
`docs/GENESIS.md` and the re-genesis checklist are updated in the same PR; the DECISIONS
entries of 2026-11-07 and 2026-11-08 stay as written, and the entry of 2026-11-22 supersedes
their "exactly the minimum" if Eric takes B.

Tests: `a_bond_other_than_the_genesis_bond_is_refused_and_named` and
`the_genesis_bond_outlasts_ten_downtime_slashes_and_a_smaller_margin_does_not`
(`xtask/src/genesis_check.rs`), `mainnet_bonds_must_be_exactly_the_genesis_bond`
(`bins/genesis-ceremony/src/committee.rs`), `a_mainnet_bond_must_be_exactly_the_genesis_bond`
(`bins/genesis-ceremony/src/main.rs`); A's tests are those of the amendment above.
