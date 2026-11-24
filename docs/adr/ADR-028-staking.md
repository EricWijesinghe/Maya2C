# ADR-028: Staking — bonded committees, slot-counted liveness, evidence-based slashing

**Status:** Accepted
**Date:** 2026-09-27
**Implements:** ADR-016 launch blocker 2; Master Prompt 4 §9

## Context

ADR-027 put DAG-BFT in the node with a fixed genesis committee. Master Prompt 4
§9 asks for registration with a minimum post-quantum bond, active-set
selection, delegation, epoch rewards, and slashing with on-chain evidence for
double-signing and downtime, tested with 1,000 validators under Byzantine
faults. Master Prompt 18 asks for concentration metrics each epoch.

## Decision

### Rules in a dependency-free crate

`crates/staking` (`maya-staking`) is a pure state machine, like
`ledger-math`, `dex`, `governance` and `fee-market`: every operation returns
the balance movements (`Effect::Debit | Credit | Burn`) for the node to apply,
or refuses and changes nothing. No dependencies, so Kani can compile it
(`src/proofs.rs`). Conservation — `debits + reward pools == held + credits +
burns` — is checked after every one of the scenario test's steps.

| rule | value (devnet defaults; mainnet from Master Prompt 18) |
|---|---|
| minimum self bond | 1,000 |
| committee | top `max_validators` (100) eligible by total stake, ties by id |
| unbonding delay | 7 epochs; funds stay slashable while waiting |
| double sign | 50 % of self bond, delegations and unbonding funds; tombstoned, id retired forever |
| downtime | < 50 % of led slots committed → 1 % slashed, jailed 2 epochs |
| rewards | pro rata by stake; commission first, then self bond and delegators pro rata; dust burned |

Equal-weight voting among members: the committee is chosen by stake, but each
member holds one vote in the engine. Stake-weighted quorums would need a
weighted `Committee` in `maya-dag-bft`; recorded here as the known gap between
"chosen by stake" and "weighted by stake".

### Liveness from headers, not from a new field

Participation must be something every node computes identically and a
replaying node can check. The block header already carries the seal (ADR-027:
epoch and anchor round), and the leader of an even round is
`committee[(round / 2) mod n]`. So the per-block staking pass counts, for each
block, the slot it filled and every slot skipped since the previous block. A
skipped slot is one its leader's anchor never committed. No header change, no
unsigned system transaction, and the counts are a function of the chain.

Counting uses the epoch's committee as recorded (`k:cmt:<epoch>`), not the
live `active` list: a tombstone mid-epoch removes a validator from `active`
but not from the leader schedule the engine is running.

### Evidence is the gossip it came from

`StakingAction::ReportEquivocation` carries two `Propose` frames exactly as
gossiped. The state machine decodes them, loads the named epoch's committee and
keys, and re-verifies both ML-DSA signatures. Anyone may submit. Committees are
kept for the unbonding window, which is exactly how long evidence can still
reach an offender's funds.

### Registration proves key possession

A registration carries the validator key's signature over the sender's
address. Without it anyone could register someone else's key.

### State

One record `k:state` (the whole staking state), `k:key:<id>` per validator,
`k:cmt:<epoch>` per epoch, folded as state layer 15 ("maya staking state root
v1") between IoT and Stateless. Present only when genesis configures staking,
so no existing root moves. Held value is in the conservation equation; burns
credit the fee sink (supply conserved, circulation falls).

### Epochs and the engine

Every `epoch_blocks` blocks the pass ends the epoch. After inserting that
block, each node's driver reads the new committee from state and replaces its
engine; a key outside the new committee runs as an observer. Every node does
this after the same block. An epoch that would leave no eligible validator
keeps the previous committee — the chain halting forever is worse than a
committee governance must then repair.

## What this does not do

- **No reward source.** Pools are passed as zero until the fee market is on
  (ADR-016 blocker 3).
- **Equal-weight votes** among a stake-chosen committee (above).
- **One record** for the whole state: linear rewrite per change. Fine at
  1,000 validators; revisit before 10,000.

## Evidence

- `crates/staking/tests/scenario.rs` — 1,000 validators, 30 epochs, random
  joins, delegations, unbonds, refused operations, double-signs and downtime;
  conservation and committee rules after every step.
- `crates/node/tests/bft_staking_tests.rs` — five real chains: a registration
  joins the committee at the boundary (largest stake first) on every node;
  evidence tombstones a genesis validator and burns half its bond; the chain
  keeps finalizing with each new committee.

## Addendum 2026-10-26: stake added in an epoch earns from the next

**Reward eligibility.** Self bond and delegations added during epoch E
(`register`, `bond_more`, `delegate`) are held in `pending_self` /
`pending_delegations` and earn nothing for E. `end_epoch` pays on the earned
stake only, then clears the pending maps, so that stake earns from E+1.
`unbond` and `undelegate` take pending stake first. `slash` cuts pending at
the same rate as earned stake. Committee ranking and vote weight still use
total stake, including pending.

**Record version 2 -> 3.** The staking record now encodes the two pending
maps. This is a hard fork of the state encoding. Version 2 records are refused,
not migrated. It rides the pre-mainnet re-genesis: any network still on
version 2 (the testnet included) is re-genesised with this binary, not
upgraded in place. Genesis clears pending after the bond
registrations, so genesis bonds earn from epoch 0.

## Amendment 2026-11-24: every double-sign is slashable, every penalty is in the genesis id, and a mainnet starts with seven seats

Eric 2026-11-24 (the recommended options of the 11-24 audit decisions, each to be
presented as a guarantee).

**CON-05: an attestation double-sign costs what a proposal double-sign costs.** A block
attestation (ADR-038) signs `(epoch, height, block)`; two of them by one committee member
for two blocks at one height are what two conflicting checkpoints are made of. Before
this, the node saw such a pair and could only report it: the on-chain verifier accepted
two `Propose` frames and nothing else. `ReportEquivocation` now also accepts two
attestation frames. No wire change: an attestation frame starts with `0xA7`, which no
engine frame does, so the evidence names its own kind and a pair of one kind can never be
read as the other. The pair must name one epoch, one height, one committee index and two
blocks, and both signatures must verify against that index of the epoch's frozen
committee. The validator is then slashed `double_sign_slash_bps` and tombstoned, exactly as
for proposals (STK-15). Keeping the pair as a file the operator can submit is the node-local
part, in the consensus hardening PR (#686).

**Written staking rules (ceiling rows S1, S2, S4, S9, S13).** `unbonding_epochs`,
`double_sign_slash_bps`, `downtime_slash_bps`, `downtime_threshold_bps`, `jail_epochs` and
`max_commission_bps` came from `Params::DEVNET` whatever the genesis said, and none reached
the genesis id. `bft.staking.rules` now names all six, each required, no unknown field, and
folds them into the id (`"maya2c genesis staking rules v1"`, STK-18). A genesis without the
section keeps its id and the compiled values, so every test and testnet genesis is
unchanged. The ceremony writes the section on every value-bearing chain, as literals
(`LAUNCH_STAKING_RULES`), so a later change to the devnet defaults cannot move a mainnet;
today they equal the values the node has always run, and a ceremony test pins that.
`cargo xtask genesis-check` and the node both refuse a value-bearing genesis without them.

**C4 and C5: committee bounds the node enforces itself.** A genesis naming more validators
than `max_validators` is refused on every chain. A value-bearing genesis (chain id
containing `mainnet`, the ceremony's rule) is refused with fewer than seven validators
(`MIN_VALUE_BEARING_VALIDATORS`; seven seats survive two faults) or without a staking
section and its written rules. The ceremony's `MIN_LAUNCH_VALIDATORS` is now that same
constant, so the tool and the node cannot disagree. STK-17.

Tests: `an_attestation_double_sign_is_slashed_at_the_proposal_rate`,
`the_staking_rules_are_part_of_the_genesis_id_and_are_the_rules_run`,
`the_loader_refuses_a_committee_outside_its_bounds` (`crates/node/tests/bft_staking_tests.rs`);
`unwritten_staking_rules_fail` (`xtask/src/genesis_check.rs`);
`mainnet_needs_seven_validators_a_testnet_does_not`,
`the_launch_rules_are_the_values_the_node_has_always_run`
(`bins/genesis-ceremony/src/committee.rs`).

Open, not decided here: whether `unbonding_epochs = 7` (seven 3,600-block epochs, about
seven hours at one block a second) is the evidence window mainnet wants. It is the value
the node always ran; it is now written and hashed, so changing it is one line in
`LAUNCH_STAKING_RULES` before genesis.
