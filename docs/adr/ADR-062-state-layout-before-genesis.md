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

**Integration note (re-freeze bundle, PR #540 + #555):** ADR-062a below caches the sparse
accounts root only, and it is dark at genesis (`STATELESS` at `u64::MAX`); on the genesis
chain the accounts fold above is unchanged until Eric decides 062a's activation, and record
layers stay full rebuilds either way.

**Residual, for Eric:** the default `min_delegation` is 10 base units, so the
1 MiB record (and the old 8 MiB one) can be filled with dust for fees alone,
after which every new delegation is refused until it drains. A real minimum,
or ADR-062b, ends that. ADR-062b also removes the 10,900-delegation ceiling
this amendment introduces.

## Amendment 2026-11-14 — ADR-062a, incremental sparse accounts root (built)

Eric 2026-11-14 MCQ: "incremental root now". Built in `crates/node/src/state/root_cache.rs`.

**What it is (revised after the PR #540 review).** A persistent cache of the sparse accounts
tree's *branches*, under the local-only prefix `smt:`: one entry per node with accounts on both
sides, keyed by the node's key bits zero-padded to 32 bytes and then its depth, holding the two
children's digests as it sees them. N accounts have exactly N − 1 branches, whatever the
addresses. Single-child levels are not stored: `sparse::subtree` hashes each as
`internal(child, empty)`, and the walk folds such a run in memory (at most 256 BLAKE3 calls, no
read). The first layout stored every node holding two or more accounts, single-child levels
included, so a pair of recipients one bit apart cost about 240 entries each, and a 5 MiB block of
such pairs about 9.6M entries; the review found it by reading the code, and
`neighbour_addresses_cost_one_branch_each_not_a_spine` now pins one entry an account. The key
order puts a node before everything below it, so the topmost branch under a prefix is one seek;
a prefix with no branch has at most one account, one seek into `acct:`. Per touched key a block
costs one seek per branch on its path (about log2 N for addresses nobody chose; up to 256 for
addresses lined up along one path, each such branch an account the attacker paid for) and at
most 2 × 256 in-memory hashes; untouched siblings cost nothing, their digests being in the
parent's entry. No fixed bucket depth: recipients are free to choose, so a depth-16 bucket cache
(the 2026-10-28 sketch) is one an attacker fills a single bucket of.

**Large blocks (second revision).** A valid cache is always walked. The first revision switched to
a build over every account when touched × (bits(N) + 1) × 2 exceeded N (N ≥ 4,096), which is
about 2.8k touched keys at 100k accounts and 23.8k at 1M. That build replaced the whole cache in
the block's batch: a `delete_range` over `smt:` and all N − 1 branches (about 105 B each, so about
105 MB at 1M accounts), where the full rebuild before ADR-062a wrote nothing, and range tombstones
piling up over `smt:` slow the walk's seeks until compaction. It was also the slower path in its
own measurement (max block at 1M accounts: walk 1.70 s, rebuild 2.33 s, test profile). The walk
visits at most the union of the touched paths, so never more than the N − 1 branches a build
hashes, at one seek each (1.3-1.4 µs a seek against 1.06-1.09 µs an account of rebuild, test
profile: at most about 1.3× a rebuild), and writes only the branches that changed. A build now
runs only without a valid cache: the first commit after activation, after a withdrawal, or as a
repair. Pinned by `a_busy_block_walks_and_writes_its_paths_not_the_whole_cache` (5,000 accounts,
250 touched keys, past the old crossover: walked, at most touched × (bits + 2) cache writes).
A maximal block (81,920 new outputs) still costs seconds at 100k-1M accounts: that is the bound
for a block that touches that much, not a saving
(`reports/sessions/2026-11-14-incremental-state-root.md`). Release-profile timings: NOT MEASURED.

**Valid blocks only.** The per-block bound is for blocks whose declared root the cache gives. A
block it does not give costs a full rebuild before it is refused (`check_declared_root`, net 2
below), which is what every block cost before ADR-062a. On a proof-of-work chain the work is
checked before execution (`Chain::insert_block_inner`), so that cost is behind the price of a
block's work; a DAG-BFT chain builds every block it applies itself (`build_block`), so the
declared root is its own preview's.

**Computed once.** The plan (root, branch writes, account count) is made with the root at preview
or staging and kept in the overlay. The commit stages it if the write generation it was made at is
still current and the account changes are the same; otherwise it plans again under the write
guard. The marker and the walk read one snapshot. The generation moves twice a write, when the
write guard is taken and when it is dropped after the write lands, so a plan read without the
lock in between (generation already moved, snapshot from before the write) carries a value no
commit starts from and is never staged (`a_plan_read_while_a_write_is_landing_is_not_staged`;
PR #540 review). Before this, only the chain mutex kept that race out.

**Root value unchanged.** The value is `maya-stateless-core`'s `sparse::root` over every account,
as before. `StateDB::state_root()` stays the full rebuild and is the reference; staging
(`preview_root`, every apply path) reads the cache. Pinned by
`incremental_root_equals_full_rebuild_over_random_histories` (12 seeded histories × 80 steps of
commits, reverts and out-of-band writes; after every step the stored cache equals a fresh build
node for node).

**Atomicity.** The cache delta is staged in the same `WriteBatch` as the accounts on all four
commit paths (`apply_block`, `apply_block_checked`, `apply_journaled_with`, `revert_block_with`).
The bare key `smt:` holds the committed root the cache describes and the account count, and is
present only while it is valid: `put_account`, `raw_put` on `acct:`, and any `write_batch` that
writes `acct:` (genesis seeding, snapshot import, purge) delete it, and so does any commit whose
post-state is not sparse. `write_batch` also treats any operation its batch iterator cannot report
(a `delete_range`, a `merge`) as an account write. The next commit after a deletion rebuilds the
cache from every account, once. Snapshots never carry `smt:` (`LOCAL_ONLY_PREFIXES`,
invariant 25), so a snapshot-restored or pruned node builds its own
(`a_snapshot_carries_no_root_cache_and_the_importer_builds_its_own`).

**When the cache is wrong.** Four nets, cheapest first:

1. `Chain::open` runs `verify_root_cache`, a branch-by-branch comparison with a full rebuild
   (O(N) reads and hashes, and O(N) memory for the account list, about 72 B an account), and
   rebuilds on any difference (`chain_open_rebuilds_a_damaged_cache_before_the_first_block`).
   Measured 3.86 s and, in a second run, 4.53 s at 1M accounts (test profile), beside `check_tip_state`'s own full rebuild.
   It grows linearly: by the same per-account figures, about 40 s and 720 MB at 10M accounts
   (arithmetic, not measured).
2. A block whose declared root differs from the cache's is checked against the full rebuild
   before it is refused. If the full rebuild matches the header, the cache was wrong: it is
   withdrawn, the block is accepted on the full rebuild's root, and the next commit rebuilds the
   cache (`a_wrong_cache_costs_a_rebuild_not_the_block`). A wrong cache costs one rebuild, not a
   validator that refuses every honest block.
3. A block preview (`preview_root`: what a producer puts in its header, and what every DAG-BFT
   validator runs on every block it derives) checks the cache branch by branch against a full
   rebuild (`root_cache::holds`) once at every height that is a multiple of `CONFIRM_EVERY`
   (256), however many times a builder previews that height. On a difference the preview's plan
   is a build over every account, so the header carries the full rebuild's root and the commit
   replaces the cache (`a_damaged_cache_is_caught_at_a_confirm_height_before_a_header_carries_it`,
   `a_preview_checks_the_cache_once_at_each_confirm_height`). Cost: about one rebuild per 256
   heights. A node that applies blocks without previewing them (a follower, an RPC node, a
   validator in catch-up) runs the same check on the apply path at those heights: otherwise a
   wrong cache root equal to a block's declared root was accepted with no full check, and the node
   forked off silently (independent AI review R-4, 2026-11-21;
   `a_follower_that_never_previews_refuses_a_block_declaring_a_damaged_cache_root`). The
   contracts root cache confirms on the same import path at the same heights (PR #549 review R-4,
   ADR-142 amendment 5, `a_node_that_never_previews_refuses_a_block_declaring_a_damaged_cache_root`).
4. Debug builds compare every planned root with the full rebuild. Unit tests (`cfg(test)`) panic
   on a difference, so a cache bug fails them; any other debug build (a debug testnet node, the
   integration tests) counts a repair and plans a build over every account, as a release node
   does at net 3.

Each repair is counted (`StateDB::root_cache_stats().repaired`) and written to stderr, the node's
log. What is still not covered: a cache that goes wrong between two confirm heights. A node whose
own preview carries the wrong root (a producer, or any DAG-BFT validator deriving a block) accepts
it itself, because its kept preview matches its own header, while its peers refuse it; net 3 stops
it at the next confirm height, but blocks committed before then keep their wrong roots, and on
restart `check_tip_state` refuses to open (`StateRootMismatch`), so the node needs a re-sync. The
failure is loud, not silent. Checking every preview would close it at the cost of a full rebuild
per block on every DAG-BFT validator, which is the cost ADR-062a removes; 256 is the trade, and it
is a local constant, not consensus.

**What it does not do.**

1. **It does not change the genesis root format.** The sparse tree is still the dark
   `FeatureSet::STATELESS` feature (`STATELESS_ACTIVATION_HEIGHT = u64::MAX`); at genesis the
   accounts root is the dense address-ordered tree (`state::merkle`), and no cache is kept. The
   dense tree cannot be made incremental at the same root value: inserting an account shifts the
   position of every account after it. Removing the per-block O(N) accounts cost at genesis
   therefore means activating the sparse tree at or near genesis (the `STATELESS` feature bit in
   a genesis schedule entry, which `crates/node/tests/root_cache_chain_tests.rs` does on a test
   chain), and that also retires the dense `account_proof` the light client's SPV path verifies
   today, and needs a `spec/03-state.md` rule and vectors for the sparse root. **Decision for
   Eric, not taken here:** (a) activate at genesis, with those changes, or (b) keep ADR-062a dark
   and accept that mainnet's per-block accounts cost is unchanged by it. Until then the brief's
   goal (per-block root cost that follows the touched keys) is **not delivered on the genesis
   chain**, and the parts that run there are bookkeeping only: one `smt:` marker delete per
   commit, `put_account` taking the write guard, a probe over every `write_batch`, the declared
   root compared through `check_declared_root`, a second generation bump per write, and the
   `confirm_due` counter on each preview. None of them changes a root.
2. **Record layers stay full rebuilds.** Every `RECORD_LAYERS` prefix, channels, contracts and
   nullifiers are dense `merkle_root`s over key-ordered leaves, rebuilt every block (205-213 ms at
   100k records, test profile, `reports/sessions/2026-11-13-per-block-scan-bounds.md`). The same
   insertion-shift argument applies; making them incremental is a root-format change of its own.

## Consequences (062a)

No spec rule changes and no conformance vector changes: the root is the same function of state.
`spec/03-state.md` records that the value is the full function and an implementation may compute
it incrementally.

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

