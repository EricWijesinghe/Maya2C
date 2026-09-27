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
