# ADR-062: Choose the accounts tree and the staking record layout before genesis

Status: Accepted (2026-10-27, Eric: both, measure first). Staking (b) and accounts (b) are built behind measurement (`cargo xtask bench-tps` before and after, and block-apply time against account count); what ships in the re-genesis is what the numbers justify. Originally Proposed 2026-10-26.

## Context
The freeze-readiness optimisation review (2026-10-26) found two costs that grow
with chain size and are locked into the state-root format.

1. **Accounts tree.** `StateDB::root_with_overlay` (`state/db.rs`) re-reads every
   account each block, hashes every leaf and rebuilds the whole dense binary
   tree (`state/merkle.rs`). Cost per block is O(all accounts), paid by the
   producer and every validator. A sparse tree already exists
   (`state/stateless.rs`, `sparse::root`), switched on by
   `STATELESS_ACTIVATION_HEIGHT`, currently `u64::MAX` (`docs/stateless.md`).
2. **Staking record.** `state/staking.rs` stores every validator and delegation
   in one value under one key, decoded and re-encoded whole on every staking
   transaction and each epoch boundary. Someone who mass-delegates dust makes
   every staking transaction costlier. The value also outgrows the old 65,536
   byte decode cap (fixed in the same change as ADR-061).

## Options
- **Accounts:** (a) keep the dense tree, tune the I/O only; (b) activate the
  sparse tree from genesis, which allows incremental update and witness-based
  stateless verification, at the cost of resolving the open questions in
  `docs/stateless.md` (how witnesses travel; the end-of-block passes).
- **Staking:** (a) keep one record; (b) one record per validator and per
  delegation, key-ordered, so a transaction touches only its own records.

## Recommendation
Staking (b) and accounts (b), decided together and shipped in one re-genesis,
each behind its own measurement: `cargo xtask bench-tps` before and after, and a
block-apply time against account count (MISSING today). Do not ship a state-root
change that has not been measured.

## Consequences
Light-client proofs and `spec-ref` depend on the tree shape, so the spec and
conformance vectors change with it. The live testnet cannot follow either; both
need the planned re-genesis.

## Amendment 2026-11-15: per-delegation staking records (ADR-062b), built

Eric 2026-11-14 MCQ: "per-delegation records". Staking option (b) ships in the
re-genesis; the 2026-10-28 deferral of 062b is withdrawn.

**Layout** (`spec/06-staking.md` STK-1..STK-8, `state/staking_records.rs`):
`k:state` keeps only parameters, epoch, committee and the epoch's slot counts
(record version 4; versions 1–3 are refused). Each validator is `k:v:<id>`,
each delegation `k:d:<validator><delegator>`, each unbonding entry
`k:u:<validator><release epoch><owner>` with a release index
`k:ur:<release epoch><validator><owner>`, and rewards owed to a validator's
delegators for one epoch `k:pj:<validator><epoch>`. Validator first, so a
slash reads two key ranges and nothing else. The validator record carries
aggregates over its own records (delegation count, pending stake, unbonding
total), kept exact as differences over what each operation loaded.

**Rules unchanged.** `maya-staking` still owns every rule. The node loads the
records an operation touches into a partial `Staking` and runs the same
function on it. The one addition is `Staking::end_epoch_deferred`. It is
`end_epoch` with two changes: delegators are owed instead of credited at
once, and earning delegated stake comes from the aggregates. Both paths share
`pay_rewards`, so the arithmetic is the same code.

**Delegators are paid later, in the same amounts.** A delegation's share of
epoch E is `rest × (a − p) / total`, the value `end_epoch` credits at the
boundary. It is paid the first time anything reads that delegation, or when
the per-block pass reaches it (1,024 delegations a block), whichever comes
first. What rounding leaves is burned once nobody is owed. Total burned equals
the old dust. The change users see: a reward is spendable some blocks after
the boundary, not in the boundary block.

**Bounds.**
- A staking transaction reads its own records only.
- A boundary reads every validator, the offenders' delegations and unbonding
  entries, and the entries due.
- A plain block reads the meta record plus at most 1,024 delegations.
- A new delegation record is refused once the chain holds
  `1,024 × epoch_blocks / 2` of them, so payouts drain in half an epoch.
  That is 1,843,200 at 3,600-block epochs. The 8 MiB record cap on master and
  the 1 MiB cap from #522 are no longer needed.

**Not bounded by this change, reported:**
1. A slash reads every delegation and unbonding entry of its offender. That is
   proportional to the offender's records, not to the chain's.
2. The staking layer's root still folds every `k:` record each block. That is
   the general state-root cost PR #522's 2026-11-13 amendment records (ADR-062a). Per
   record it is now about one leaf per delegation where it was one leaf for
   all of them.
3. `min_delegation` (10 base units by default) is still Eric's decision. The
   ceiling above stops a dust crowd from making the payout backlog unbounded,
   but a crowd that reaches the ceiling still blocks *new* delegation records
   until some leave.

Measured figures: `reports/sessions/2026-11-15-per-delegation-staking.md`.
