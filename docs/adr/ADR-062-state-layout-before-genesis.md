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

**Large blocks.** When touched × (bits(N) + 1) × 2 exceeds N (and N is at least 4,096), the plan
builds from every account instead of walking: at that size the seeks cost more than the rebuild.
Both give the same root. The measured seek-to-account cost ratio is 1.3 to 1.4 (test profile,
100k and 1M accounts); it is rounded up to 2 because the rebuild's cost does not depend on the
addresses a block touches and the walk's does. A maximal block (81,920 new outputs) costs
1.3-2.3 s either way at 100k-1M accounts: that is the bound, not a saving
(`reports/sessions/2026-11-14-incremental-state-root.md`).

**Computed once.** The plan (root, branch writes, account count) is made with the root at preview
or staging and kept in the overlay. The commit stages it if the write generation it was made at is
still current and the account changes are the same; otherwise it plans again under the write
guard. The marker and the walk read one snapshot.

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

**When the cache is wrong.** Three nets, cheapest first:

1. `Chain::open` runs `verify_root_cache`, a branch-by-branch comparison with a full rebuild
   (O(N) reads and hashes, and O(N) memory for the account list, about 72 B an account), and
   rebuilds on any difference (`chain_open_rebuilds_a_damaged_cache_before_the_first_block`).
2. A block whose declared root differs from the cache's is checked against the full rebuild
   before it is refused. If the full rebuild matches the header, the cache was wrong: it is
   withdrawn, the block is accepted on the full rebuild's root, and the next commit rebuilds the
   cache (`a_wrong_cache_costs_a_rebuild_not_the_block`). A wrong cache costs one rebuild, not a
   validator that refuses every honest block.
3. Debug builds compare every planned root with the full rebuild, so every test that runs the
   sparse tree checks the cache.

Each repair is counted (`StateDB::root_cache_stats().repaired`) and written to stderr, the node's
log. What is not covered: a producer with a wrong cache in a release build proposes a block whose
root its peers refuse, and accepts it itself (its kept preview matches its own header). That node
forks itself off. On restart `check_tip_state` compares the full rebuild with the tip's header and
refuses to open (`StateRootMismatch`), so the failure is loud, not silent, and the node needs a
re-sync. The repair count is the signal to watch; a periodic deep check is not built.

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
   and accept that mainnet's per-block accounts cost is unchanged by it. Until then this change
   has no runtime effect on the genesis chain beyond one marker delete per block.
2. **Record layers stay full rebuilds.** Every `RECORD_LAYERS` prefix, channels, contracts and
   nullifiers are dense `merkle_root`s over key-ordered leaves, rebuilt every block (205-213 ms at
   100k records, test profile, `reports/sessions/2026-11-13-per-block-scan-bounds.md`). The same
   insertion-shift argument applies; making them incremental is a root-format change of its own.

## Consequences (062a)

No spec rule changes and no conformance vector changes: the root is the same function of state.
`spec/03-state.md` records that the value is the full function and an implementation may compute
it incrementally.
