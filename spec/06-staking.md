# 6. Staking and slashing — v0.1.2

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

- **STK-1** The staking records are `k:state` (parameters, epoch, committee, the epoch's slot counts, the number of delegation records and of unbonding entries), `k:v:<validator>`, `k:d:<validator><delegator>`, `k:u:<validator><release epoch><owner>` (an unbonding entry's amount), `k:ur:<release epoch><validator><owner>` (empty: the release index) and `k:pj:<validator><epoch>` (rewards owed to that validator's delegators for that epoch). A `k:state` whose version byte is not 4 is refused; a single-record state (versions 1–3) is never reinterpreted. *(positive only: defines a value)* The byte layout of every value is the table below; `spec/tests/staking.json` gives fixed bytes for each kind.
- **STK-2** A validator record carries its delegation record count, the pending part of its delegations and of its self bond for the epoch it names (zero in any other), the sum of its unbonding entries and their number; each equals the sum over its own records after every operation, and the meta record's two counts equal the delegation and unbonding records in state. No unbonding entry of amount zero is stored: one a slash cuts to zero is deleted with its release index entry, since releasing it credits nothing and the rules already ignore it when deciding whether to keep its validator. A record whose pending part exceeds the stake it belongs to, a zero delegation, or a payout owed to nobody does not decode. `spec/tests/staking.json` gives the staking layer root and the state root of a fixed state holding one record of each kind.
- **STK-3** A staking transaction reads the meta record, the validator it names, the delegation it names and that validator's open payouts, and the one unbonding entry it may merge into. A report of equivocation decodes its two frames, reads the committee record of the epoch they name and the validator record at the author's index in it: against an unknown or already tombstoned validator it is refused there, before any validator key is read or any signature verified. That refusal makes the transaction invalid, where the single-record rules accepted it as a no-op; the change is intended, so a public equivocation pair cannot be replayed into blocks at a block builder's cost. Otherwise it reads the committee's keys, verifies the two signatures, and reads every delegation and unbonding entry of the offender as two key ranges: at most the offender's positions (STK-9) plus its operator's own unbonding entries. Nothing else is read, so its cost does not depend on any other validator's delegations. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-4** An epoch boundary reads every validator record, the delegations and unbonding entries of each validator slashed for downtime, and the release index up to the new epoch, which is at most every open unbonding entry (STK-7). Each active validator's earning delegated stake is its `delegated` less its pending part; no delegation of a validator that is not slashed is read. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-5** At the boundary of epoch E a paid validator's operator is credited as before, and its reward less that credit is owed to its delegators as `k:pj:<validator><E>` with the delegation count at the boundary. A delegation that was `a` at the boundary, `p` of it added in E, receives `rest × (a − p) / total` rounded down — the amount the single-record rules credited at the boundary — the first time an operation reads it or the per-block pass reaches it, whichever is first, and exactly once. When no delegation is still owed, what is left is burned. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-6** Every block, after the boundary work, the payout pass pays at most 1,024 delegations, taking the lowest `k:pj:` key first and that validator's delegations in key order from where the last block stopped. Within a block the pass resumes after the last payout key it finished. Every delegation the pass walks is settled against every open payout of its validator; a validator's lower payout key is always finished first, and a later payout then walks from that validator's first delegation. A payout whose validator has no delegation left to pay while it still owes some is a state error, refused like a wrong root: every node halts at that block. Its precondition is the invariant that an open payout's `remaining` equals the number of its validator's delegations whose `next_payout_epoch` is at most the payout's epoch, which the randomized history test recounts after every step. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-7** A position is a delegation record or an unbonding entry. A delegation that would create a new delegation record, and an undelegation that keeps part of its delegation and would open a new unbonding entry, are refused once the chain holds `min(MAX_DELEGATION_RECORDS, 1,024 × epoch_blocks / 2)` positions, `MAX_DELEGATION_RECORDS` = 131,072. An unbonding entry keeps its position until it is released, so delegating and leaving at once frees nothing. Never refused for this: adding to an existing delegation, an undelegation folding into the owner's entry for the same release epoch, an undelegation of the whole delegation (it trades its record for at most one entry), and a validator's own unbond (at most one entry per release epoch, so at most `unbonding_epochs` open per validator; STK-12 bounds them chain-wide). Positions therefore exceed the ceiling only by operators' own entries: up to the reserve of STK-12, plus full operator exits of at least `min_self_bond`, which STK-12 exempts and capital bounds. The staking layer holds at most twice the positions in delegation, unbonding and release-index records, plus validators, payouts and the meta record. The second term means that, while the per-block pass spends its budget on the payouts of one boundary, it reaches every delegation within half an epoch. That is the typical case, not a bound proved under churn: the budget is also spent on delegations created after the boundary (which owe nothing) and on payouts left open from earlier epochs, so a payout can outlive the next boundary; it stays correct, because open payouts accumulate and each delegation is paid every one it is owed. The first term bounds the staking layer's per-block root fold and what one boundary releases (Gaps). *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-8** Native coin held by staking is the sum, over its records, of each validator's self bond, each delegation, each unbonding entry and each payout still owed; the conservation check folds exactly these. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-9** The operations STK-7 refuses chain-wide are refused for one validator once its record counts `MAX_DELEGATIONS_PER_VALIDATOR` = 16,384 positions (`MAX_DELEGATION_RECORDS / 8`), with the same exemptions. It bounds what one double-sign slash or one downtime boundary reads, settles and rewrites at 16,384 positions plus `unbonding_epochs` operator entries. Eight validators full to this ceiling fill the chain-wide one exactly; seven leave room. *(external vectors: crates/node/src/state/staking_store_tests.rs)*
- **STK-10** Committing an operation rewrites a delegation record only when its meaning changed: its amount, its pending part in the current epoch, or its `next_payout_epoch`; and a validator record only when its rules record, its pending parts in the current epoch, its counts or its unbonding sum changed, so a boundary that changes no validator writes none. Settling a delegation that owes no open payout leaves `next_payout_epoch` as it was (no payout for an earlier epoch can be created later). A slash or a downtime boundary that cuts nothing from a delegation writes nothing for it. *(external vectors: crates/node/src/state/staking_store_tests.rs)*

- **STK-12** An operator's unbond that would open a new unbonding entry is refused once the chain holds `ceiling + OPERATOR_ENTRY_RESERVE` positions, where the ceiling is STK-7's and `OPERATOR_ENTRY_RESERVE` = 16,384 (`MAX_DELEGATIONS_PER_VALIDATOR`). Never refused for this: an operator unbond folding into its own entry for the same release epoch (it adds no position), and a full exit, one that leaves nothing bonded, of at least `min_self_bond`, so an operator can always leave. An operator whose bond a slash took under `min_self_bond` tops up to it (a top-up is never refused) and then exits in full. A full exit under the minimum is not exempt: otherwise one validator could top up one base unit and leave again every epoch. Delegators are refused at the ceiling (STK-7), so only operators can use the reserve. Each exempt entry holds at least `min_self_bond` until it releases, so positions never exceed `ceiling + 16,384 + staked coins / min_self_bond` whatever the number of registered validators (at most 131,072 + 16,384 + 19,687 = 167,143 at the mainnet 1,000-coin bond if every coin the halving eras issue were bonded): that bounds what one boundary releases (STK-4). STK-11 is reserved for the committee-floor rule of a parallel change (ADR-070). Pending Eric (decision brief 2, 09 R-7). *(external vectors: crates/node/src/state/staking_store_tests.rs)*

## Downtime and the committee floor

- **STK-11** At an epoch boundary a seated validator is down when it authored fewer than `downtime_threshold_bps` of the anchor slots it led (authored × 10,000 < threshold × expected, in integers); a validator with no slots is never down, and one still serving a jail term is unpaid but not punished again. Down validators that are eligible now are ordered worst first (authored ÷ expected, compared as authored_a × expected_b against authored_b × expected_a), ties by id. The floor keeps the last `min(down eligible, max(0, MIN_COMMITTEE − (eligible validators not down)))` of them, `MIN_COMMITTEE` = 4, counted **before** any slash. A kept validator is unpaid and not jailed, and is slashed `downtime_slash_bps` of its self bond only when what is left is at or above `min_self_bond`. Every other down validator is slashed and jailed; one already ineligible is always jailed. (ADR-070 and its amendments 2026-11-21 and 2026-11-22.) *(external vectors: crates/staking/tests/scenario.rs)*

The genesis bond margin (every mainnet seat bonds `min_self_bond` plus 10.6%,
ADR-070 amendment 2026-11-22) is a launch policy checked by
`cargo xtask genesis-check`, not a consensus rule: the node does not check
the margin.

## Record layouts

Byte strings are concatenated in the order listed. `u8`/`u16`/`u64` and
amounts (256-bit, 32 bytes) are little-endian (ADR-046); an epoch inside a
*key* is 8 bytes big-endian so keys sort by epoch. Ids, operators,
delegators and owners are 32 bytes. Fixed bytes for every kind:
`spec/tests/staking.json`.

| Record | Key | Value |
|---|---|---|
| Meta | `k:state` | `u8` version = 4; params (`min_self_bond` amount, `min_delegation` amount, `max_validators` u16, `unbonding_epochs` u64, `double_sign_slash_bps` u16, `downtime_slash_bps` u16, `downtime_threshold_bps` u16, `jail_epochs` u64, `max_commission_bps` u16); `epoch` u64; `active` as u64 count then that many ids; `epoch_blocks` u64; `last_round` u64; `expected` as u64 count then (id, u64) pairs in id order; `authored` the same; `delegation_records` u64; `unbonding_records` u64 |
| Validator | `k:v:` ‖ id | `operator`; `self_bond` amount; `delegated` amount; `commission_bps` u16; status (`u8` tag: 0 bonded, 1 jailed followed by `until` u64, 2 tombstoned); `pending_epoch` u64; `pending_self` amount; `pending_delegated` amount; `delegations` u64; `unbonding` amount; `unbonding_entries` u64 |
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
  with positions up to `MAX_DELEGATION_RECORDS` (STK-7), and up to twice
  that many records when every position is an unbonding entry
  (`scan_bounds_tests::staking_cost_against_delegation_records` and
  `staking_cost_against_unbonding_entries`, report
  `reports/sessions/2026-11-15-per-delegation-staking.md`). That is the
  general state-root cost (ADR-062 amendment of 2026-11-13); ADR-062a's
  incremental root covers accounts only. Raising the ceiling waits for an
  incremental staking-layer root. Accepting this for genesis is open for
  Eric (DECISIONS.md 2026-11-15, review round 2).
- PARTIAL, a boundary's release of due unbonding entries is proportional to
  the entries due, bounded by the ceiling, the operator reserve and the
  capital-bounded full operator exits (STK-4, STK-7, STK-12), not by a per-block budget: every position released
  at one boundary is the measured worst case.
- PARTIAL, validator count. Registering a validator has no record ceiling
  (`max_validators` limits only the active set); only `min_self_bond`, locked
  and refundable, limits it. Every boundary reads every validator record, so
  that read grows with registered validators. At the mainnet 1,000-coin
  floor that is at most 2,625 validators in the first year if every coin
  issued were bonded (about 2,625,000 coins issued a year in era 1 at one
  block a second) and at most 19,687 over all four halving eras. The
  positions operators open no longer grow with it (STK-12). A registration
  ceiling needs a validator count in the meta record, a layout change; open
  for Eric.
- PARTIAL, dust, pending Eric (decision brief 2 option A). The proposed
  mainnet `min_delegation` is 100 coins (10^26 base units, enforced by
  `cargo xtask genesis-check`; it was 1 coin, DECISIONS.md 2026-10-30); the
  devnet default is 10 base units. A crowd of 16,384 minimum positions closes
  one chosen validator to new delegators: 1,638,400 coins locked at the
  proposed floor (16,384 at 1 coin), refunded after the unbonding window.
  131,072 positions, eight such crowds, close every new delegation record on
  the chain: 13,107,200 coins (131,072 at 1 coin), more than every coin
  issued in the first five years (about 11,812,500 at one block a second). Top-ups, delegators' exits and full operator exits of at least
  `min_self_bond` always pass. Filling the operator reserve (STK-12) takes
  2,341 validators (16,384 / 7 unbonding epochs) at 1,000 coins each,
  2,341,000 coins, kept refilled at every boundary. For as long as that fill
  is kept up, an operator's partial unbond is refused, not only delayed: a
  slot freed at a boundary can be taken first by the attacker in the next
  block. A full exit still passes, so an operator held by the fill can leave
  (topping up to `min_self_bond` first if a slash took it under), and its
  downtime-slash exposure ends with the exit. Evicting
  the smallest position at a full ceiling (option B) is the follow-up after
  genesis; a deposit per record or a higher ceiling behind an incremental
  root are the others.
- STK-3 to STK-10 and STK-12 are about which records exist and are read, which the
  node's own tests check against the in-memory rules; their JSON vectors are
  the layouts and the root only.
