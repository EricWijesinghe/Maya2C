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
   heights.
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
