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

## Amendment 2026-11-13: per-block work against stored records

The 2026-10-28 deferral of both halves (DECISIONS.md) assumed state grows
organically and said to revisit "when accounts pass ~50k or block time is
threatened". A sweep of every end-of-block pass and the root, after PR #500
found the same pattern in governance, measured it against records an attacker
creates. Test profile (node at opt-level 0), median of 3,
`state::scan_bounds_tests`, report `reports/sessions/2026-11-13-per-block-scan-bounds.md`.

**Fixed in this change** (`fix/per-block-scan-bounds`, no key layout change):

| Site | Before | Bound now | Measured at the bound |
|---|---|---|---|
| `settle_sealed`: one reveal height could inherit 255 × 128 = 32,640 envelopes, and `merged_records` walked the whole overlay per envelope | 18.95 s at 32,640 | `MAX_SEALED_PER_REVEAL_HEIGHT` = 128, checked at submission; `Overlay::records_under` ranges the overlay | 0.39 ms at 128 (183 ms at 32,640 with the range alone) |
| `settle_trading` rebuilds every touched book whole; the per-book ceiling times the pairs one block touches was the real bound | 549 ms for 16 full books; a block at the fee target touches about 190 | `MAX_BOOK_ORDERS_PER_BLOCK` = 16,384 resting orders across touched books; the first book is always admitted | 86 ms for 4 full books |
| The staking record is one value, decoded and re-encoded by every block and every staking transaction; the cap was 8 MiB | 87,000 delegations (8.35 MB): 250–289 ms a block, 152 ms a transaction | record cap 1 MiB (about 10,900 delegations); `MAX_STAKING_TX_PER_BLOCK` = 16, evidence exempt (both removed by the 2026-11-15 amendment below, ADR-062b) | 10,000 delegations (0.96 MB): 28 ms a block, 15 ms a transaction |

Each refusal is a transaction failing, never a block. Rejected: a counter
record per reveal height (a new key for a bound the scan of at most 128 keys
already gives); skipping the book pass for pairs touched only by swaps (changes
matching semantics, and a placement touches a pair as cheaply); caching the
decoded staking record across a block (`select_applicable` clones the overlay
per non-transfer transaction, so the clone would cost what the decode did).

**Not fixed here, CRITICAL:** the state root itself. `root_with_overlay`
re-reads and hashes every account and every committed record each block:
1.3–1.8 µs an account and 2.1 µs a 200-byte record on the test profile
(131 ms at 100,000 accounts), 0.48 µs an account in release
(`reports/12-performance.md`). One output to a fresh address creates an
account; a 4 MiB transfer carries about 65,000 outputs, so 2.1 million
accounts (about 1 s of root in release) take some 26 full blocks, and the fee
from the floor costs well under a millionth of a coin (about 1,000 coins even
at the compiled ceiling). That is ADR-062a (an incremental root) or a
state-creation price, both decisions for Eric. Every per-record bound above
still leaves its records in this fold.

**Residual, for Eric:** the default `min_delegation` is 10 base units, so the
1 MiB record (and the old 8 MiB one) can be filled with dust for fees alone,
after which every new delegation is refused until it drains. A real minimum,
or ADR-062b, ends that. ADR-062b also removes the 10,900-delegation ceiling
this amendment introduces.

## Amendment 2026-11-15: per-delegation staking records (ADR-062b), built

Eric 2026-11-14 MCQ: "per-delegation records". Staking option (b) ships in the
re-genesis; the 2026-10-28 deferral of 062b is withdrawn.

**Layout** (`spec/06-staking.md` STK-1..STK-10 with byte layouts, vectors `spec/tests/staking.json`, `state/staking_records.rs`):
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

**Bounds** (revised after the first and second reviews of PR #555, same day).
- A staking transaction reads its own records only. A report of equivocation
  against an unknown or already tombstoned validator is refused after reading
  the committee record and that validator's record, before any key is read or
  signature verified (second review), so a public equivocation pair cannot be
  replayed into blocks at the builder's cost (STK-3). On master such a report
  was a valid no-op; making it invalid is intended.
- A boundary reads every validator, the offenders' delegations and unbonding
  entries, and the entries due.
- A plain block reads the meta record plus at most 1,024 delegations.
- **Chain-wide ceiling:** a new delegation record is refused once the chain
  holds `min(131,072, 1,024 × epoch_blocks / 2)` of them (STK-7). The first
  draft used `1,024 × epoch_blocks / 2` alone, 1,843,200 at 3,600-block
  epochs. That counted only execution: the staking layer's root still folds
  every `k:` record each block, so the old 8 MiB record's limit (about 87,000
  delegations) had also been a limit on the root, and dropping it would have
  let the root grow about 21-fold. 131,072 keeps the root near master's worst
  case: measured on the test profile, the state root over 131,072 delegation
  records takes 142.8 ms (100,000: 102.0 ms; 10,000: 9.0 ms), against
  250–289 ms a block for master's 87,000-delegation record in
  `settle_staking` alone (the 2026-11-13 amendment above). Release profile:
  MISSING.
- **Per-validator ceiling:** a new delegation record to a validator holding
  16,384 is refused (STK-9). It bounds what one slash or downtime boundary
  reads, settles and rewrites: a double-sign slash over 16,384 delegations
  takes 69.3 ms, and a downtime boundary over the same crowd 75.5 ms, on the
  test profile. A crowd has to fill eight validators to reach the chain-wide
  ceiling. Since the second review both ceilings count positions (next item).
- **Positions, not delegation records** (second review). Both ceilings
  count a validator's delegation records plus its unbonding entries, and an
  entry keeps its position until it is released. Before this, a fresh
  address could delegate the minimum and leave at once: the delegation record
  went, the entry stayed, and neither ceiling ever filled, so the entries
  (two records each) grew without bound into the slash, the boundary's
  release and the root. Refused at a full chain or validator: a new
  delegation record, and a partial undelegation that would open a new entry.
  Never refused: a top-up, a full undelegation (its record becomes the
  entry), folding into this epoch's entry, and an operator's unbond (one
  entry per release epoch, at most `unbonding_epochs` open). A slash now
  reads at most 16,384 positions plus those operator entries; a boundary
  releases at most every position; the root folds at most twice the
  positions in records. Measured on the test profile with every one of
  131,072 positions an unbonding entry: state root 238.6 ms, the boundary
  that releases them all 797.9 ms (262,153 records read), and the
  release-index front over the 131,072 tombstones left behind 8.4 ms
  (`scan_bounds_tests::staking_cost_against_unbonding_entries`); with the
  node at opt-level 3 (no LTO, not the release profile) 167.2 ms and
  367.6 ms, and the root over 131,072 delegations 103.0 ms. An entry a
  slash cuts to zero is deleted with its index entry, so no record outlives
  its counted position. `StoredValidator` gains `unbonding_entries` and the
  meta record `unbonding_records` (both u64, vectors in
  `spec/tests/staking.json`).
- A delegation record is rewritten only when its meaning changes (STK-10). A
  downtime cut of zero from 16,384 dust delegations writes none of them
  (16,384 before this rule). The same holds for validator records (second
  review): a boundary that changes no validator rewrote every one, because
  the stored pending epoch always moved; now none.
- `Params::is_valid` refuses `min_delegation` = 0 (second review): it admits
  `delegate(0)`, a zero delegation record that does not decode, so every
  later read of that validator would halt the chain.
- The 8 MiB record cap on master, and #522's 1 MiB cap and 16 staking
  transactions a block, are removed: #522 is merged into this PR. The 64th
  staking transaction of a block reads what the first does.

**Not bounded by this change, reported:**
1. **PARTIAL: per-block cost independent of total delegations.** Execution
   is; the state root is not. The root folds every staking record each block,
   up to the chain-wide ceiling. An incremental staking-layer root (ADR-062a
   extended past accounts) removes this, and only then should the ceiling
   rise, by an upgrade. Accepting PARTIAL for genesis is open for Eric
   (DECISIONS.md 2026-11-15, review round 2); it is not marked met.
2. A slash or downtime boundary still does its offender's work in one block,
   bounded now by the per-validator ceiling, not by the chain's. Likewise the
   boundary that releases due unbonding entries does them all in one block,
   bounded by the chain's ceiling (797.9 ms at the worst case, test profile);
   a budgeted release pass, like the payout pass, would bound it per block
   but changes when funds arrive and what a slash can still reach, so it is
   not done here.
3. **Dust lockout (PARTIAL, open for Eric; amendment 062c below proposes a
   100-coin floor).** The mainnet floor was decided:
   `min_delegation` = 1 coin, required at the ceremony and enforced by
   `cargo xtask genesis-check` (DECISIONS.md 2026-10-30); 10 base units is
   only the devnet default. At that floor, 16,384 positions close one chosen
   validator to every new delegator: 16,384 coins locked, refunded after the
   unbonding window. This is a targeted attack on honest validators, a new
   competitor among them, and the per-validator ceiling added it. 131,072
   positions (eight such crowds; seven leave room) close every new delegation
   record on the chain: 131,072 coins locked. Top-ups and exits always pass.
   A per-record deposit, a floor that rises as the ceiling fills, or an
   incremental staking root that lets the ceiling rise would change this;
   each changes economics, so it is Eric's call (DECISIONS.md 2026-11-15,
   review round 2).
4. The payout pass's drain within half an epoch is the typical case, not a
   proved bound under churn (STK-7); a payout that outlives the next boundary
   stays correct.

Measured figures: `reports/sessions/2026-11-15-per-delegation-staking.md`.

## Amendment 2026-11-22 (062c): dust floor and operator-entry reserve, pending Eric

Status: PROPOSED, pending Eric (decision brief 2 and review finding R-7 of
the 2026-11-19 AI review; that review is AI work, not an external audit).
Built as a draft PR stacked on #555, never merged without his choice.

**Floor (brief 2, option A).** Every position holds at least
`min_delegation`: a delegation and each part of a partial exit are refused
below it. The proposed mainnet floor is 100 coins (10^26 base units,
`xtask/src/genesis_check.rs` `MIN_DELEGATION`), up from 1 coin. A genesis
value only: no record format or rule changes.

| Floor | Close one validator (16,384 positions) | Close the chain (131,072) |
|---|---|---|
| 1 coin (decided 2026-10-30) | 16,384 coins | 131,072 coins |
| 10 coins | 163,840 coins | 1,310,720 coins |
| **100 coins (proposed)** | **1,638,400 coins** | **13,107,200 coins** |
| 1,000 coins | 16,384,000 coins | 131,072,000 coins (more than the 19,687,500 the halving eras issue) |

For scale: era 1 issues about 2,625,000 coins a year at one block a second
(`deploy/genesis/mainnet/README.md`), so at 100 coins closing the chain takes
more than every coin issued in the first five years. The locked capital is not
lost: it earns rewards and is refunded after the unbonding window, so the
attacker's cost is opportunity cost. The price of the higher floor is that a
holder with less than 100 coins cannot delegate directly and needs a pool.
Options B (evict the smallest position at a full ceiling; after genesis, by an
upgrade), C (a refundable per-record deposit) and D (keep 1 coin) are in the
brief and in the PR body.

**Operator-entry reserve (R-7).** Checked against the code: an operator
`Unbond` never called `refuse_new_position`, and `Staking::register` has no
count cap, so positions exceeded the ceiling by up to registered validators ×
`unbonding_epochs`, with no rule bounding the validator count. Now an operator
unbond that would open a new entry is refused once positions reach
`ceiling + OPERATOR_ENTRY_RESERVE` (16,384, one validator's worth), spec
STK-12; folding into its own entry for the same release epoch is never
refused, and neither is a full exit (nothing left bonded) of at least
`min_self_bond`, so an operator can always leave; one a slash took under the
minimum tops up to it first (a top-up is never refused). A full exit under
the minimum is not exempt, or one validator could top up one base unit and
exit again every epoch. Each exempt entry holds at least `min_self_bond`
until it releases, so positions are bounded by `ceiling + 16,384 + staked
coins / min_self_bond` (at most 167,143 at the 1,000-coin bond with every
coin the halving eras issue bonded) whatever the number of validators, which
bounds what one boundary releases. Delegators are refused at the ceiling, so
only operators reach the reserve. Filling it takes 2,341 validators (16,384 /
7 unbonding epochs) at 1,000 coins each, 2,341,000 coins kept refilled at
every boundary. For as long as the fill is kept up, operators' partial
unbonds are refused, not merely delayed (the attacker can take a freed slot
first in the next block); full exits still pass. The first draft of this
amendment refused full exits too and said the fill "only delays"; review of
the draft found that wrong, and the exemption replaced it. Not done: a registration ceiling (it needs a validator
count in the meta record, a layout change); validator records stay bounded by
`min_self_bond` alone, at most 2,625 in the first year and 19,687 over the
halving eras if every coin issued were bonded.

