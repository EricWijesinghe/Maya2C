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
  (`Checkpoint::verify_weighted`, `Collector::add_weighted`, and
  `catchup::fetch` with weights). A catching-up node imports blocks on the
  checkpoint quorum alone. Counted in heads, that quorum would let cheap
  seats attest a chain of their own making.

Evidence (`crates/node/tests/bft_staking_tests.rs`, 5 passed):
- `absent_cheap_seats_halt_a_one_seat_one_vote_chain`: the control. Three
  absent 1,000-bond seats beside four 10,000-bond validators stop the chain.
- `absent_cheap_seats_cannot_halt_a_stake_weighted_chain`: the same attack
  against a weighted genesis. The chain keeps producing, and epoch 1's
  recorded weights are exactly the seven stakes.
- `stake_weighting_is_part_of_the_genesis_id`.
- The ceremony's `only_a_value_bearing_chain_is_stake_weighted` covers the
  launch configuration.

Still open (gate 10): recovery from a halt caused by more than a third of
the stake going offline.
