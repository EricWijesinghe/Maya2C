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

- **STK-1** The staking records are `k:state` (parameters, epoch, committee, the epoch's slot counts, the number of delegation records), `k:v:<validator>`, `k:d:<validator><delegator>`, `k:u:<validator><release epoch><owner>` (an unbonding entry's amount), `k:ur:<release epoch><validator><owner>` (empty: the release index) and `k:pj:<validator><epoch>` (rewards owed to that validator's delegators for that epoch). A `k:state` whose version byte is not 4 is refused; a single-record state (versions 1–3) is never reinterpreted. *(positive only: defines a value)* The byte layout of every value is the table below; `spec/tests/staking.json` gives fixed bytes for each kind.
- **STK-2** A validator record carries its delegation record count, the pending part of its delegations and of its self bond for the epoch it names (zero in any other), and the sum of its unbonding entries; each equals the sum over its own records after every operation. A record whose pending part exceeds the stake it belongs to, a zero delegation, or a payout owed to nobody does not decode. `spec/tests/staking.json` gives the staking layer root and the state root of a fixed state holding one record of each kind.
- **STK-3** A staking transaction reads the meta record, the validator it names, the delegation it names and that validator's open payouts, and the one unbonding entry it may merge into. A report of equivocation reads the offender's validator record first: against an unknown or already tombstoned validator it is refused (the transaction is invalid) having read nothing else; otherwise it also reads every delegation and unbonding entry of the offender, as two key ranges, at most `MAX_DELEGATIONS_PER_VALIDATOR` delegations (STK-9). Nothing else is read, so its cost does not depend on any other validator's delegations. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-4** An epoch boundary reads every validator record, the delegations and unbonding entries of each validator slashed for downtime, and the release index up to the new epoch. Each active validator's earning delegated stake is its `delegated` less its pending part; no delegation of a validator that is not slashed is read. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-5** At the boundary of epoch E a paid validator's operator is credited as before, and its reward less that credit is owed to its delegators as `k:pj:<validator><E>` with the delegation count at the boundary. A delegation that was `a` at the boundary, `p` of it added in E, receives `rest × (a − p) / total` rounded down — the amount the single-record rules credited at the boundary — the first time an operation reads it or the per-block pass reaches it, whichever is first, and exactly once. When no delegation is still owed, what is left is burned. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-6** Every block, after the boundary work, the payout pass pays at most 1,024 delegations, taking the lowest `k:pj:` key first and that validator's delegations in key order from where the last block stopped. Within a block the pass resumes after the last payout key it finished. Every delegation the pass walks is settled against every open payout of its validator; a validator's lower payout key is always finished first, and a later payout then walks from that validator's first delegation. A payout whose validator has no delegation left to pay while it still owes some is a state error, refused like a wrong root: every node halts at that block. Its precondition is the invariant that an open payout's `remaining` equals the number of its validator's delegations whose `next_payout_epoch` is at most the payout's epoch, which the randomized history test recounts after every step. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-7** A delegation that would create a new delegation record is refused once the chain holds `min(MAX_DELEGATION_RECORDS, 1,024 × epoch_blocks / 2)` of them, `MAX_DELEGATION_RECORDS` = 131,072; adding to an existing delegation is never refused for this. The second term means that, while the per-block pass spends its budget on the payouts of one boundary, it reaches every delegation within half an epoch. That is the typical case, not a bound proved under churn: the budget is also spent on delegations created after the boundary (which owe nothing) and on payouts left open from earlier epochs, so a payout can outlive the next boundary; it stays correct, because open payouts accumulate and each delegation is paid every one it is owed. The first term bounds the staking layer's per-block root fold (Gaps). *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-8** Native coin held by staking is the sum, over its records, of each validator's self bond, each delegation, each unbonding entry and each payout still owed; the conservation check folds exactly these. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-9** A delegation that would create a new delegation record to a validator is refused once that validator's record counts `MAX_DELEGATIONS_PER_VALIDATOR` = 16,384 delegation records (`MAX_DELEGATION_RECORDS / 8`); adding to an existing delegation is never refused for this. It bounds what one double-sign slash or one downtime boundary reads, settles and rewrites, and it means a crowd must fill eight validators to reach the chain-wide ceiling. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-10** Committing an operation rewrites a delegation record only when its meaning changed: its amount, its pending part in the current epoch, or its `next_payout_epoch`. Settling a delegation that owes no open payout leaves `next_payout_epoch` as it was (no payout for an earlier epoch can be created later). A slash or a downtime boundary that cuts nothing from a delegation writes nothing for it. *(external vectors: crates/node/src/state/staking_store_tests.rs)*

## Record layouts

Byte strings are concatenated in the order listed. `u8`/`u16`/`u64` and
amounts (256-bit, 32 bytes) are little-endian (ADR-046); an epoch inside a
*key* is 8 bytes big-endian so keys sort by epoch. Ids, operators,
delegators and owners are 32 bytes. Fixed bytes for every kind:
`spec/tests/staking.json`.

| Record | Key | Value |
|---|---|---|
| Meta | `k:state` | `u8` version = 4; params (`min_self_bond` amount, `min_delegation` amount, `max_validators` u16, `unbonding_epochs` u64, `double_sign_slash_bps` u16, `downtime_slash_bps` u16, `downtime_threshold_bps` u16, `jail_epochs` u64, `max_commission_bps` u16); `epoch` u64; `active` as u64 count then that many ids; `epoch_blocks` u64; `last_round` u64; `expected` as u64 count then (id, u64) pairs in id order; `authored` the same; `delegation_records` u64 |
| Validator | `k:v:` ‖ id | `operator`; `self_bond` amount; `delegated` amount; `commission_bps` u16; status (`u8` tag: 0 bonded, 1 jailed followed by `until` u64, 2 tombstoned); `pending_epoch` u64; `pending_self` amount; `pending_delegated` amount; `delegations` u64; `unbonding` amount |
| Delegation | `k:d:` ‖ validator ‖ delegator | `amount` amount; `pending` amount; `pending_epoch` u64; `next_payout_epoch` u64 |
| Unbonding entry | `k:u:` ‖ validator ‖ release epoch (be) ‖ owner | `amount` amount |
| Release index | `k:ur:` ‖ release epoch (be) ‖ validator ‖ owner | empty |
| Payout | `k:pj:` ‖ validator ‖ epoch (be) | `rest` amount; `total` amount; `owed` amount; `remaining` u64; cursor (`u8` tag: 0 none, 1 followed by the last delegator paid) |

A value with bytes left over, a status or cursor tag outside the ones above,
a zero delegation, a pending part above the stake it belongs to, or a payout
with `remaining` = 0 or `total` = 0 does not decode.

## Gaps

- The staking rules (ADR-028) have no rule IDs yet; `crates/staking/tests/scenario.rs`
  and the Kani harnesses are their evidence.
- PARTIAL, per-block cost independent of total delegations. Execution
  (what staking reads and writes in a block) is independent of total
  delegations: STK-3, STK-4, STK-6. The state root is not: the staking
  layer's root still folds every `k:` record each block, so its cost grows
  with delegation records up to `MAX_DELEGATION_RECORDS` (STK-7), whose value
  is set from the measured root cost (`scan_bounds_tests::staking_cost_against_delegation_records`,
  report `reports/sessions/2026-11-15-per-delegation-staking.md`). That is the
  general state-root cost (ADR-062 amendment of 2026-11-13); ADR-062a's
  incremental root covers accounts only. Raising the ceiling waits for an
  incremental staking-layer root.
- STK-3 to STK-10 are about which records exist and are read, which the
  node's own tests check against the in-memory rules; their JSON vectors are
  the layouts and the root only.
