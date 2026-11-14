# 6. Staking and slashing — v0.1.0

Staking is built (ADR-028): bonding, delegation, committee selection,
downtime and double-sign slashing, epoch rewards. Reference: `crates/staking`
(the rules, `no_std`, Kani harnesses in `src/proofs.rs`); node:
`crates/node/src/state/{staking,staking_records,staking_store,staking_exec}.rs`.
The rules themselves (minimums, slash rates, the committee floor, reward
split) are specified by `crates/staking` and its tests, not yet as rule IDs
here. This section pins how the node **stores** that state, which since
ADR-062b is one record per validator, per delegation, per unbonding entry and
per owed payout, so a block's staking work does not grow with the number of
delegations. Every key is under the staking layer's prefix `k:` and so under
the state root (ROOT-5, invariant 25). Amounts are 256-bit little-endian
(ADR-046); epochs in keys are big-endian so they sort.

- **STK-1** The staking records are `k:state` (parameters, epoch, committee, the epoch's slot counts, the number of delegation records), `k:v:<validator>`, `k:d:<validator><delegator>`, `k:u:<validator><release epoch><owner>` (an unbonding entry's amount), `k:ur:<release epoch><validator><owner>` (empty: the release index) and `k:pj:<validator><epoch>` (rewards owed to that validator's delegators for that epoch). A `k:state` whose version byte is not 4 is refused; a single-record state (versions 1–3) is never reinterpreted. *(positive only: defines a value)* *(external vectors: crates/node/src/state/staking_records.rs)*
- **STK-2** A validator record carries its delegation record count, the pending part of its delegations and of its self bond for the epoch it names (zero in any other), and the sum of its unbonding entries; each equals the sum over its own records after every operation. A record whose pending part exceeds the stake it belongs to, a zero delegation, or a payout owed to nobody does not decode. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-3** A staking transaction reads the meta record, the validator it names, the delegation it names and that validator's open payouts, and the one unbonding entry it may merge into; a report of equivocation also reads every delegation and unbonding entry of the offender, as two key ranges. Nothing else is read, so its cost does not depend on any other validator's delegations. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-4** An epoch boundary reads every validator record, the delegations and unbonding entries of each validator slashed for downtime, and the release index up to the new epoch. Each active validator's earning delegated stake is its `delegated` less its pending part; no delegation of a validator that is not slashed is read. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-5** At the boundary of epoch E a paid validator's operator is credited as before, and its reward less that credit is owed to its delegators as `k:pj:<validator><E>` with the delegation count at the boundary. A delegation that was `a` at the boundary, `p` of it added in E, receives `rest × (a − p) / total` rounded down — the amount the single-record rules credited at the boundary — the first time an operation reads it or the per-block pass reaches it, whichever is first, and exactly once. When no delegation is still owed, what is left is burned. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-6** Every block, after the boundary work, the payout pass pays at most 1,024 delegations, taking the lowest `k:pj:` key first and that validator's delegations in key order from where the last block stopped. A payout whose validator has no delegation left to pay while it still owes some is a state error, refused like a wrong root. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-7** A delegation that would create a new delegation record is refused once the chain holds `1,024 × epoch_blocks / 2` of them, so the pass finishes every epoch's payouts within half an epoch; adding to an existing delegation is never refused for this. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-8** Native coin held by staking is the sum, over its records, of each validator's self bond, each delegation, each unbonding entry and each payout still owed; the conservation check folds exactly these. *(external vectors: crates/node/src/state/staking_store_tests.rs)*

## Gaps

- The staking rules (ADR-028) have no rule IDs yet; `crates/staking/tests/scenario.rs`
  and the Kani harnesses are their evidence.
- The staking layer's root still folds every `k:` record each block, so the
  root's cost grows with delegations even though execution does not. That is
  the general state-root cost (ADR-062 amendment of 2026-11-13, ADR-062a), not
  a staking rule.
- No JSON vectors: these rules are about which records exist and are read,
  which the node's own tests check against the in-memory rules.
