# ADR-040: Stake-weighted DAG-BFT committees

**Status:** Accepted (2026-10-04).
- **Part 1, the engine:** merged in #59. It changes no running behaviour.
- **Part 2, the node wiring:** approved by Eric on 2026-10-04, active from
  a new genesis (option 2 below). The genesis ceremony turns it on for
  every value-bearing chain. maya-testnet-1 stays on equal weights with
  `--min-register-bond`.

**Date:** 2026-10-04

## Context

ADR-039 left mainnet gate 10 open: **committee capture**.

- Staking (ADR-028) chooses the committee by stake.
- The engine then gives every member one vote.
- A seat costs `min_self_bond`.

So registrations that never come online can take more than a third of the
*seats*, and stop every quorum. Against four honest validators two such
seats are enough. A halted chain never reaches the epoch boundary that would
jail them.

The textbook answer, and what stake-weighted BFT systems do, is to count
votes by stake. An attacker then needs a third of the *stake*, which means
buying it.

## Decision

### Part 1: the engine counts weight (built)

`maya_dag_bft::Committee` carries one weight per member:
- `Committee::weighted(weights)`.
- `Committee::new(n)` is `n` equal weights.

Every threshold is measured in weight. With W the total weight:
- f = ⌊(W − 1)/3⌋.
- Quorum is W − f. This is ADR-039's rule, measured in weight.
- Validity is f + 1.

| Check | Was | Now |
|---|---|---|
| Votes certify a vertex | count ≥ q | weight of voters ≥ q |
| A vertex's parents | count ≥ q, in `is_well_formed` | weight of the parents' authors ≥ q, checked where the DAG can name them (`Validator::has_parent_quorum`): on a proposal, and on every certificate before it enters the DAG |
| A round is complete | certificates ≥ q | weight of the round's authors ≥ q |
| An anchor commits | linking vertices ≥ f + 1 | their authors' weight ≥ f + 1 |
| A committee of one certifies alone | q ≤ 1 | the node's own weight is a quorum |

Weights are integers and sums are `u128`. A zero weight is mapped to one
inside the engine. Part 2 must hand every node the same snapshot, and the
mapping then gives every node the same committee. Nothing here is a float or depends
on iteration order, so every node computes the same thresholds (Standing
Order 4). A zero weight counts as one, so every member can be heard.

**Leaders stay round-robin over members.** An absent member still costs one
anchor timeout (1 s) per turn until the epoch boundary jails it. That costs
throughput, not safety or liveness. Leader election by stake would remove
the cost, but it also changes which validator orders which transactions, and
it does not belong in the same change.

With equal weights every threshold is exactly what it was, and honest
operation is unchanged by Part 1. The node still builds
`Committee::new(size)`. Two checks did become stricter, and only a faulty
sender notices either one:
- A vertex's parents count by *distinct author in the previous round*,
  resolved through the DAG. Two parents by one author, or a parent from
  another round, no longer add up to a quorum. Before, the count was of
  digests.
- The cheap check before any signature is verified is now a head-count
  floor: the fewest members whose weight can reach a quorum. With equal
  weights that floor is the old n − f.

At or below the garbage-collection horizon a certificate's parents are
gone, so they cannot be weighed. There only the floor applies, which is the
exemption `Dag::missing_parents` already makes. Without that exemption, a
late certificate at the horizon would be refused for ever and a rejoining
node would stall. Both reviews found this, and it is fixed.

### Part 2: the node supplies stake as weight (built 2026-10-04)

Weights must be identical on every node for a whole epoch. Live stake is
not: `bond_more`, `delegate`, `unbond` and equivocation slashing all change
a validator's total in the middle of an epoch. Two nodes, one restarted
before a delegation and one after, would read different weights and accept
different certificates.

The weights must therefore be a **snapshot taken at the epoch boundary**,
stored in state beside the committee (`k:cmt:<epoch>`). That snapshot is a
new state record (invariant 25). It changes the state root from the first
boundary that writes it. So a chain adopts it in one of two ways:

1. **At an activation height** written into each chain's configuration.
   Before it, equal weights and no record; from it, the snapshot. Nodes
   replaying history reproduce old roots. This is more code, and keeps
   maya-testnet-1 and its soak running.
2. **At genesis** of a new chain, mainnet included. This is less code. For
   maya-testnet-1 it means a re-genesis, which restarts the gate-3 soak.

Recommendation: (2) for mainnet, whose genesis has not happened, and leave
maya-testnet-1 on equal weights with `--min-register-bond` (ADR-039
option 3) until it is next re-genesised for another reason. Part 2 also
needs the ADR-038 checkpoint quorum (`attest.rs`, `quorum_of`) to count
weight. It currently builds `Committee::new(size)` too.

## What this does not fix

- **Recovery from a halt** (ADR-039). If more than a third of the *stake* is
  offline, the chain still stops, as BFT must. It still has no way back
  short of operators restarting it. That is a separate design.
- **A sybil with real stake.** Weighting prices a seat at its stake. Whoever
  holds a third of the stake can still stop the chain. That is the
  intended security assumption, not a gap.

## Evidence

- `cargo test -p maya-dag-bft`:
  - lib 16 passed, including
    `a_seat_bought_at_the_minimum_bond_cannot_block_a_quorum` and
    `weighted_quorums_always_overlap_in_more_than_the_faulty_weight`
    (64 committees of pseudo-random stakes).
  - `--test modes_sim` 10 passed, including
    `three_cheap_absent_seats_cannot_stop_the_bonded_validator` (three of
    four seats crashed, and the bonded validator commits alone) and
    `honest_stake_split_in_half_still_halts_rather_than_forks`.
- `cargo check --workspace --all-targets`: clean.
- Reviewed by the rust-reviewer and security-reviewer agents (2026-10-04).
  - Both found the GC-horizon refusal (HIGH and MEDIUM), which is fixed.
  - Also fixed: the `BTreeSet` allocation in `weight_of` (now a flat
    bitmap), a panic on an oversized committee (`weighted` now returns
    `Option`), and the dropped cheap pre-verification parent check (now the
    head-count floor).
  - `a_certificate_at_the_collection_horizon_is_accepted_without_its_parents`
    (`tests/auth_and_restart.rs`) pins the horizon fix. With the exemption
    removed, it fails ("a horizon certificate was refused").
- `cargo nextest run -p custom-l1-node -p maya-dag-bft -p maya-link-sim -p maya2c-node`:
  1064 tests run: 1064 passed (1 slow), 2 skipped, 86.585 s.

## Part 2 as built

- **Genesis.** `bft.staking.stake_weighted`, which defaults to off and is
  hashed into the genesis id when on. The ceremony sets it for
  `maya-mainnet`.
- **Weights.** Epoch 0's weights are written at genesis under
  `k:cmw:<epoch>`, inside the staking layer, so under the state root. Each
  epoch boundary freezes the new committee's stakes (self bond plus
  delegations) only if the previous epoch had weights, so a chain never
  gains the record unless its genesis asked for it. Weights are pruned
  with the committee records.
- **Engine.** The driver builds `Committee::weighted` from the epoch's
  record, at boot and at every epoch switch.
- **Checkpoints (ADR-038).** These count stake too
  (`Checkpoint::verify`, `Collector::add`, and
  `catchup::fetch` with weights). A catching-up node imports blocks on the
  checkpoint quorum alone. Counted in heads, that quorum would let cheap
  seats attest a chain of their own making. Both take `weights` as an
  argument with no default, so a caller cannot forget it.
- **One snapshot.** The committee's keys come from the epoch's frozen
  committee record (`committee_ids`), the same snapshot as its weights. A
  mid-epoch tombstone shrinks `active`, but it changes neither the keys nor
  the weights the engine runs until the next boundary.
- **Followers.** `follow_loop` re-reads the committee and weights on every
  round (`BftDriver::staked_committee`), so it checks each checkpoint
  against the current epoch, not the one it started in.

Evidence (`crates/node/tests/bft_staking_tests.rs`, 6 passed):
- `absent_cheap_seats_halt_a_one_seat_one_vote_chain`: the control. Three
  absent 1,000-bond seats beside four 10,000-bond validators stop the chain.
- `absent_cheap_seats_cannot_halt_a_stake_weighted_chain`: the same attack
  against a weighted genesis. The chain keeps producing, and epoch 1's
  recorded weights are exactly the seven stakes.
- `stake_weighting_is_part_of_the_genesis_id`.
- `a_weighted_node_restarts_after_a_mid_epoch_tombstone_and_keeps_weights`:
  the review's HIGH (keys from `active`, weights frozen at the boundary).
  - Without the fix, it fails with "committee weights do not match the
    committee".
  - It also crosses three more boundaries and checks that the weights are
    still recorded.
- `attest::tests::cheap_seats_make_a_head_quorum_but_never_a_stake_quorum`
  covers the checkpoint quorum. Three cheap signers make a quorum by heads
  but not by stake, and weights of the wrong length are an error.
- The ceremony's `only_a_value_bearing_chain_is_stake_weighted` covers the
  launch configuration.

Accepted for v1, from the security review:
- **Tombstones.** A validator tombstoned mid-epoch keeps its frozen weight
  until the boundary. Its key can still sign for the rest of that epoch.
  Zeroing its weight mid-epoch would change the quorum under an epoch in
  flight, which is the bug fixed above.
- **Boundary timing.** Stake moved in the block before a boundary is
  weighted for the next epoch. Unbonding delays already stop stake from
  leaving and returning within one epoch. Weighting by the minimum of two
  epochs' stake is a candidate hardening, not built.
- **Zero stake.** A zero stake counts as weight 1, the engine's floor.

Still open (gate 10): recovery from a halt caused by more than a third of
the stake going offline.

## Amendment 2026-11-24: no operator holds more than a fifth of the vote (CON-06)

Eric 2026-11-24: cap consensus weight per operator at 20%, below the third that could stop
finality; stake above the cap still earns. Before this the 20% cap was a genesis check
(`cargo xtask genesis-check`) only, and `BondMore` or `Delegate` could move any share of the
weight onto one operator after launch.

`stake_weights` now ends in `state::weight_cap::cap_operator_weights`: weights are summed per
operator (the `operator` address the stake pays out to, so one operator running several keys
is one operator), and the heaviest are cut by integer water-filling to the level `c` at
which each holds at most 20% of the new total (STK-16 gives the arithmetic). A cut
operator's keys are scaled alike. Integer math only, `BTreeMap` order, ties by address.
Rewards are paid on stake by `maya-staking` and never read these weights, so excess stake
still earns in full; it just cannot dominate a vote.

The cap is a property of every epoch's weights, whatever path the stake took. It needs at
least five operators (five times 20% is all of it); with fewer, no weights can satisfy it and
they are left as stakes. A value-bearing genesis seats at least seven validators
(ADR-028 amendment 2026-11-24) and genesis-check holds each operator to 20% at launch.
With equal bonds, as at mainnet launch, the cap moves nothing.

What it guarantees, exactly: no single operator *address* holds more than 20% of an epoch's
weight. What it costs, measured with the STK-16 arithmetic (review of PR #704, 2026-11-24):
water-filling hands a cut operator's excess to every other address in proportion, so once
there are five or more addresses the weight an attacker can reach depends on how many
addresses it runs, not on how much stake it holds. Addresses cost one minimum bond each.

| Committee (one key per address at the minimum bond `m`, except one honest address large enough to be cut) | Attacker's share of the weight |
|---|---|
| 1 honest large address (> 2.75 m), 6 honest, attacker 5: level 2.75 m of 13.75 m | 36.4%: above 1/3, can stop finality |
| 1 honest large address (> 9.25 m), 6 honest, attacker 31: level 9.25 m of 46.25 m | 67.0%: above 2/3, can break safety |
| 1 honest large address (> m), attacker 4: level m of 5 m | 80% |

Without the cap the same attacker needs more than 1/3 (or 2/3) of the total stake. So, as
built, the cap lowers the cost of an attack by many small addresses against a committee
with one large honest operator. Its arithmetic is exact at every stake size (256-bit
intermediate, `the_cap_holds_at_the_full_u64_range`); the trade-off above is a property of
the design, not of rounding. Eric decided the second threshold on 2026-11-24 (third
round): a quorum needs `W − f` of the capped weight *and* two thirds of the seats by count.
That is the amendment below. It is not the raw-stake threshold this paragraph first
proposed, and it does not change the rows above (see "What the seat threshold does not
change" below).

Tests (`crates/node/src/state/weight_cap.rs`): `equal_bonds_are_left_exactly_as_they_are`,
`a_whale_is_cut_to_a_fifth_of_the_new_total_and_nobody_else_moves`,
`one_operator_running_many_keys_is_capped_as_one`,
`below_five_operators_no_cap_is_satisfiable_so_none_is_applied`,
`the_cap_holds_at_the_full_u64_range` (weights at the u64 ceiling, where `w × c` passes
2^128), and the 2,000-committee sweep `no_operator_ever_holds_more_than_the_cap` over every
bit width from 1 to 64.

## Amendment 2026-11-24 (third round): a quorum is two thirds of the stake and two thirds of the seats

Eric 2026-11-24, third round: "consensus needs BOTH 2/3 of stake weight AND 2/3 of validator
count (defeats splitting stake over many addresses; the 20% cap stays)." Built in
`maya_dag_bft::Committee` and stated as CON-20 (`spec/05-consensus.md`).

### The rule

With `W` the committee's total weight (the capped stake weights on a stake-weighted chain,
all 1 otherwise), `n` its members, `f_W = ⌊(W − 1)/3⌋` and `f_n = ⌊(n − 1)/3⌋`:

| Threshold | Before | Now |
|---|---|---|
| Quorum (`Committee::is_quorum`) | weight ≥ `W − f_W` | weight ≥ `W − f_W` **and** members ≥ `n − f_n` |
| Validity (`Committee::is_validity`) | weight ≥ `f_W + 1` | weight ≥ `f_W + 1` **and** members ≥ `f_n + 1` |
| Head-count floor (`min_quorum_size`) | fewest heaviest members reaching `W − f_W` | the larger of that and `n − f_n` |

Every place that counts a quorum already went through `Committee`, so one change reaches all
of them, and they cannot disagree:

- the engine: votes certify a vertex, a round is complete, a vertex's parents suffice
  (`validator.rs`), a certificate is well formed (`vertex.rs`), an anchor commits (`commit.rs`,
  now `is_validity`);
- the node: the driver builds `Committee::weighted` from the epoch's record (`driver.rs`),
  certificate replay checks each certificate's votes through it (`certsync.rs`), and finality
  checkpoints count through it (`attest.rs`, `has_quorum`, used by `Checkpoint::verify` and
  `Collector::add`, and so by catch-up);
- the light client verifies checkpoints with the node's `Checkpoint::verify` (ADR-089);
- the spec reference (`maya-spec-ref`, `dual_quorum`, `dual_validity`) and the TypeScript
  verifier (`spec/verifier-ts/verify.ts`) recompute CON-20 from the spec text, and
  `consensus_conformance.rs` replays the same vectors against `Committee`.

With equal weights `W = n` and `f_W = f_n`, so both thresholds are the same number and an
equal-weight chain behaves exactly as before.

Integer arithmetic only (`u128` sums of `u64` weights, `usize` counts), no floats, no map
iteration: `tally` walks the member list once with a flat bitmap.

### Why it is still safe: quorum intersection in two measures

Let `Q1` and `Q2` be quorums of one committee.

- By weight: `w(Q1 ∩ Q2) ≥ w(Q1) + w(Q2) − W ≥ 2(W − f_W) − W = W − 2f_W ≥ f_W + 1`, since
  `W ≥ 3f_W + 1`. This is ADR-039's argument in weight, unchanged.
- By seats: `|Q1 ∩ Q2| ≥ 2(n − f_n) − n = n − 2f_n ≥ f_n + 1`, since `n ≥ 3f_n + 1`.

So two quorums share more than `f_W` of the weight *and* more than `f_n` of the seats. If the
faulty members hold at most `f_W` of the weight, the shared weight cannot all be faulty; if
they hold at most `f_n` of the seats, the shared seats cannot all be faulty. Either way the
two quorums share an honest member, who never signs two vertices for one slot or two blocks
at one height. Safety therefore holds under the **weaker** of the two assumptions: an
adversary must exceed a third of the weight *and* a third of the seats to make two
conflicting certificates or checkpoints. Before this amendment it had to exceed a third of
the weight only.

The commit rule keeps its argument. An anchor committed directly has validity support in the
next round; any later anchor's parents are a quorum of that round. Weight alone gives
`(W − f_W) + (f_W + 1) > W`, so the two sets share an author and the later anchor has a path
to the committed one. The seat dimension only adds a second such overlap. Requiring validity
in both measures means the support contains an honest member under either assumption, the
same "some of it honest" the weight rule gave.

Liveness: a quorum forms when the honest, online members hold at least `W − f_W` of the weight
and `n − f_n` of the seats, that is when the faulty or absent side holds at most a third by
*each* measure.

### What it guarantees

Stated with its cost, after the dual-quorum review of 2026-11-25 (issue 2), which showed the
first wording ("stake on a few addresses finalizes nothing alone") held only for a committee
that does not grow.

- **To finalize alone a party needs two thirds of the weight and two thirds of the voters.**
  Weight alone, however concentrated, is never enough: one party holding 80% of the weight on
  four addresses of ten, each at the 20% cap, has a weight quorum and four of the seven seats a
  quorum needs, so no certificate, no checkpoint, no light-client update.
- **Seats are priced and rate-limited.** Each seat costs at least `min_self_bond` while the
  committee has free seats, and more than the smallest seated stake once it is full. Seats
  join at most `entry_limit` per epoch (STK-23), and a seat counts only once it is in the
  committee, so a party that adds seats to its stake must keep them online through each
  boundary while it builds up to two thirds. Measured on the example above with eleven
  minimum-bond seats on eleven fresh payout addresses: without STK-23 all eleven are seated at
  the next boundary, 15 of 21 seats and 811 of the 675 weight needed, an attacker-only quorum
  (the review's computation, not rerun here); with it, after 6 epochs, at 19 seats, every attacker seat online
  throughout (`cheap_seats_added_to_capped_stake_reach_no_quorum_in_one_epoch`, measured
  2026-11-25). At the 3,600-block testnet epoch and one block a second that is six hours.
  This is a delay with a visible, on-chain build-up, not a prohibition.
- **Seats bought without stake finalize nothing alone.** Unchanged from part 1: cheap seats are
  heads without weight.
- **A seat that never comes online is jailed before it can stop finality** (STK-23, from seven
  seats up): seats that join in one epoch hold at most half of `f_n`, and the boundary that
  follows jails and slashes the absent ones (STK-11).
- **A held tombstone has no say** (third round 5): weight 0 is no voter, so it spends no seat
  fault and its signature never stands in for a voter's.
- **Normal finality is unaffected.** Equal weights give the same thresholds as before; the
  committee with a member absent still commits while the absent side is within a third by
  seats and by weight.

### The price, measured: absent seats past a third halt a weighted chain again

ADR-040 part 1 made the bonded validators a quorum by stake whatever the cheap seats did. The
seat threshold takes that back. Registrations that never come online, holding more than a
third of the *seats*, now stop every quorum on a stake-weighted chain too, as they did on the
one-seat-one-vote chain ADR-039 described. The chain halts; it does not fork (both tests
below assert that the nodes still agree). A halted chain reaches no epoch boundary, so the
downtime jail that would remove the seats does not run.

| Committee | Before (weight only) | Dual, as first built | Dual + STK-23 |
|---|---|---|---|
| 4 online at 10,000, 3 absent at 1,000 registered in one epoch | goes on: 40,000 of 43,000 | halts: 4 seats of 7 | goes on: one joins per epoch, 4 of 5 |
| 4 online at 10,000, 1 absent at 1,000 (5 seats, `n − f_n` = 4) | goes on | goes on | goes on |
| 1 bonded at 10^9, 3 absent at 1,000 (sim, fixed committee) | goes on | halts | halts if seated; above the four-seat floor STK-23 seats one newcomer per epoch, and only a refill below the floor seats more |
| 3 bonded at 10^9, 1 absent at 1,000 (sim) | goes on | goes on | goes on |

What bounds it: seats are the top `max_validators` eligible validators by stake (ADR-028), so
once the committee is full a seat costs more than the smallest seated stake, not the minimum
bond; and every seat costs at least `min_self_bond`.

**STK-23, the entry limit (dual-quorum review 2026-11-25, issue 1).** The review showed the
table's first row is a deliberate, cheap and permanent halt, not only an accident: at the
genesis size, seven online at 10,000 and four registrations at the minimum that never run give
`n` = 11, `n − f_n` = 8 and seven answering seats. Nothing is committed, so the epoch never
ends, `punish_downtime` never jails them, the bonds are never slashed, and a `--trusted-block`
restart keeps the same committee. At most `entry_limit(seated) = max(MIN_COMMITTEE − seated,
⌊f_n(seated) ÷ 2⌋)` (no `max(1, …)` term since 2026-11-28: below seven seats nobody joins) validators that were not in the ending committee now join at a
boundary (`maya_staking::entry_limit`, `Staking::next_committee`). From seven seats up the
newcomers of one epoch hold at most half of `f_n`, so absent ones cannot stop finality, and the
boundary after their first epoch jails and slashes them while the next one joins. Both rows
above that halted (equal weight and stake weighted, four online and three absent) now go on
through two boundaries (`absent_cheap_seats_join_one_per_epoch_so_*`).

**What STK-23 does not close, for Eric before genesis.** A patient attacker runs its seats
honestly until it holds more than `f_n`, then goes dark. The limit makes that take several
epochs of service (from a seven-seat genesis, four epochs:
`a_patient_attacker_needs_several_epochs_of_service_to_hold_a_third`), but each seat still
costs only `min_self_bond` while seats are free, and the halt is still permanent because a
halted chain reaches no boundary. Under weight only, the same halt needed a third of the
weight. Options, with evidence:

| Option | Closes the patient halt? | Keeps decision 3? | Evidence |
|---|---|---|---|
| (a) entry limit (built, STK-23) | delays it (4 epochs from 7 seats) | yes | `committee_entry.rs` |
| (b) only seats with capped weight ≥ a floor (e.g. `W ÷ 3n`) count toward `n − f_n` | raises the cost to about an eighth of the honest weight at equal honest stakes, not a third | partly: the floor moves with `W`, so a heavy party can push many small honest seats under it and hold every counted seat | arithmetic below |
| (c) count seats per operator (the cap's grouping) | no | yes | the operator is the payout address the registrant names; the tests above use a fresh address per seat and the weight cap treats each as a separate operator |
| raise `min_self_bond` or a seat floor at election | in proportion to the price | yes | economics, not built |
| weight-only quorum (part 1), decision 3 reversed | yes, back to a third of the weight | no | ADR-040 part 1 |

Arithmetic for (b): with `h` honest seats at stake `H` and `k ≥ h/2` absent seats at weight `x`,
the floor `x ≥ (hH + kx) ÷ 3(h + k)` gives `x ≥ H/4`, so the halt costs `k·x ≈ hH/8`. The same
floor at `W ÷ n` restores a third of the weight but then the example of the first guarantee
fails: the capped addresses are the only seats above the average and hold every counted seat.
A seat count can only veto stake if small seats count, and if small seats count, small seats
can halt. That trade is the owner's to make; this amendment builds (a) and does not choose
between the rest.

### What the seat threshold does not change

The rows of the CON-06 table above are attacks by many minimum-bond addresses. Such an
attacker holds at least as large a share of the seats as of the weight (its addresses are at
the minimum bond, and water-filling lifts every uncut address alike), so the seat threshold
does not stop it: 31 addresses of 38 are 81.6% of the seats. Against that attack the cost
remains the number of seats, as the table states. The seat threshold closes the opposite
case, stake concentrated on few seats past the per-address cap.

### Interaction with the held tombstone

Third-round decision 5 gives a tombstoned validator held in its seat to keep the four-seat
floor (STK-14) zero consensus weight. The first build of this amendment wrote the opposite into
CON-20 ("a weight of 0 counts as 1") and still counted the held seat as a full seat, so at the
floor (`n` = 4, `f_n` = 1) the proven equivocator used up the one seat fault and any single
honest outage halted the chain (review 2026-11-25, issue 3). Now zero consensus weight means
no say by either measure: `Committee::weighted` keeps a 0 as 0, `n` is the number of voters
(members of positive weight, `Committee::voters`), a zero-weight member adds nothing to a
tally's weight or seats, and a committee whose weights are all 0 is refused. The member keeps
its index and its leader slot. This matches the decision-5 lane (#707, `voters`/`voters_in`,
STK-22), which sets the held tombstone's weight to 0 in `stake_weights`; the two PRs use the
same names so they merge as one rule. CON-20, `maya-spec-ref`, `verify.ts` and the vectors
`quorum-zero-weight-seat-is-no-voter`, `quorum-voters-around-a-zero-weight-seat`,
`quorum-zero-weight-seat-spends-no-seat-fault`, `validity-zero-weight-seat-is-no-second-voter`
and `quorum-committee-of-zero-weights-refused` changed together; the vector
`quorum-zero-weight-counts-as-one` is gone.

### Evidence

Red first: with the seat check absent, `cargo test -p maya-dag-bft --lib` gave
`FAILED. 19 passed; 5 failed` on the new tests, and with only the seat check removed from
`is_quorum` (a mutation, restored), `FAILED. 22 passed; 2 failed`
(`neither_stake_alone_nor_seats_alone_make_a_quorum`,
`stake_split_over_a_few_capped_addresses_cannot_finalize_alone`).

Tests:
- `crates/dag-bft/src/vertex.rs`: `neither_stake_alone_nor_seats_alone_make_a_quorum`,
  `stake_split_over_a_few_capped_addresses_cannot_finalize_alone`,
  `dual_quorums_overlap_in_more_than_the_faulty_weight_and_the_faulty_seats`,
  `validity_needs_more_than_the_faulty_weight_and_the_faulty_seats`,
  `equal_weights_make_the_two_quorums_one`,
  `the_head_count_floor_is_n_minus_f_for_equal_weights` (updated);
- `crates/dag-bft/tests/property_tests.rs`: both property tests now check the seat
  thresholds too;
- `crates/dag-bft/tests/modes_sim.rs`: `stake_on_a_few_heavy_seats_cannot_commit_alone`,
  `a_bonded_majority_of_seats_goes_on_without_an_absent_cheap_seat`, and
  `three_absent_seats_of_four_halt_even_the_bonded_validator_but_never_fork` (was
  `three_cheap_absent_seats_cannot_stop_the_bonded_validator`);
- `crates/node/src/consensus/bft/attest.rs`:
  `stake_on_a_few_seats_never_finalizes_without_two_thirds_of_the_seats`;
- `crates/node/tests/bft_staking_tests.rs`:
  `absent_cheap_seats_past_a_third_of_the_seats_halt_a_weighted_chain_too` (was
  `absent_cheap_seats_cannot_halt_a_stake_weighted_chain`) and
  `an_absent_cheap_seat_within_a_third_of_the_seats_cannot_halt_a_weighted_chain`;
  `a_node_catches_up_by_checkpoints_across_a_boundary_that_shrinks_the_committee` now
  shrinks five seats to four (one absent seat), because the three absent seats of seven it
  used before halt the chain before the boundary under this rule (it failed at
  "no second boundary" until changed);
- `crates/light-client/tests/finality_tests.rs`: `stake_on_a_few_seats_is_never_a_checkpoint_quorum`;
- CON-20 vectors in `spec/tests/consensus.json` (22 cases after the review fixes), replayed by
  `crates/node/tests/consensus_conformance.rs` and `spec/verifier-ts/verify.ts`.

Review fixes (2026-11-25), red first against the pre-fix sources (`cargo test -p maya-dag-bft
--lib vertex`: `FAILED. 15 passed; 2 failed`; `cargo test -p maya-staking --test
committee_entry`: `FAILED. 2 passed; 4 failed`, the unlimited election seating 40 of 47 in
one epoch), then green with the fix:
- `crates/dag-bft/src/vertex.rs`: `a_zero_weight_seat_is_seated_but_never_counted`,
  `a_committee_with_no_weight_is_refused`; `crates/dag-bft/tests/property_tests.rs`:
  `zero_weight_seats_change_neither_threshold`;
- `crates/staking/tests/committee_entry.rs` (STK-23):
  `absent_minimum_bond_seats_never_pass_a_third_of_the_committee` (the review's seven plus
  four), `the_entry_limit_keeps_half_the_seat_faults_for_honest_ones`,
  `a_committee_below_the_floor_refills_past_the_limit`, `a_first_committee_is_not_limited`,
  `a_richer_newcomer_displaces_at_most_the_limit_from_a_full_committee`,
  `a_patient_attacker_needs_several_epochs_of_service_to_hold_a_third` (the residual,
  measured);
- `crates/node/tests/bft_staking_tests.rs`: `absent_cheap_seats_join_one_per_epoch_so_an_equal_weight_chain_goes_on`
  and `absent_cheap_seats_join_one_per_epoch_so_a_weighted_chain_goes_on` (the two halting
  tests, now asserting finality through two boundaries), and
  `cheap_seats_added_to_capped_stake_reach_no_quorum_in_one_epoch` (issue 2, measured);
- CON-20 vectors for zero-weight seats, STK-23 in `spec/06-staking.md`.

Consensus-visible: certificates and checkpoints that were valid under weight alone may now be
refused, and committee elections admit newcomers at the STK-23 rate. This is a re-freeze change
and needs a re-genesis.
