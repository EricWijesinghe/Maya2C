# 2026-11-14 — Incremental sparse accounts root (ADR-062a)

Brief: "incremental root now" (Eric 2026-11-14 MCQ), re-freeze bundle 2. Branch
`feat/incremental-state-root` from `origin/master` 3735252d.

> **Superseded in part.** "What was built", "Red, then green", "Measured" and
> "Other runs" describe the first run. The revision below replaced its layout
> (branches only, not every node with two or more accounts), carried the plan to
> the commit instead of computing the delta twice, and made the "visited nodes"
> figures obsolete. The second revision at the end removed the crossover. Read
> the revisions for the current design.

## What was built

`crates/node/src/state/root_cache.rs`: a persistent cache of the sparse accounts
tree's interior (every node with two or more accounts) under the local-only
prefix `smt:`. Staging reads it and rehashes only the touched accounts' paths;
all four commit paths stage its delta in the block's own `WriteBatch`;
out-of-band account writes withdraw it; `Chain::open` checks it node by node
against a full rebuild. `StateDB::state_root()` stays the full rebuild. Design
and limits: ADR-062 amendment "ADR-062a".

## Findings that contradict the brief (reported, not worked around)

1. At genesis the accounts root is the **dense** tree, not the sparse one
   (`STATELESS_ACTIVATION_HEIGHT = u64::MAX`). The dense tree cannot be made
   incremental at the same root value. The cache does nothing until the sparse
   tree is active, and activating it at genesis is Eric's call (it changes the
   genesis root format and retires the dense SPV `account_proof`).
2. Record layers, channels, contracts and nullifiers are full dense rebuilds
   and stay so. Not touched.

## Red, then green

Red: a mutation in `Sub::join` (a branch with one empty child returned
unhashed). Two of the new tests failed:

```
test state::root_cache::tests::incremental_root_equals_full_rebuild_over_random_histories ... FAILED
test state::root_cache::tests::the_cache_survives_a_restart_and_a_damaged_one_is_rebuilt_on_open ... FAILED
assertion `left == right` failed: seed 0 step 2
test result: FAILED. 3 passed; 2 failed; 1 ignored; 0 measured; 543 filtered out; finished in 0.20s
```

Green (`cargo test -p custom-l1-node --lib state::root_cache -- --include-ignored --nocapture --test-threads=1`):

```
test state::root_cache::tests::a_dense_post_state_withdraws_the_cache ... ok
test state::root_cache::tests::incremental_root_equals_full_rebuild_over_random_histories ... ok
test state::root_cache::tests::root_cost_by_state_size ... ok
test state::root_cache::tests::the_cache_prefix_is_local_only_and_outside_the_root ... ok
test state::root_cache::tests::the_cache_survives_a_restart_and_a_damaged_one_is_rebuilt_on_open ... ok
test state::root_cache::tests::work_per_block_follows_touched_keys_not_total_accounts ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 543 filtered out; finished in 23.27s
```

## Measured

Test profile (the workspace's dev profile with its package overrides), Windows
11 Pro 10.0.29680, shared machine with other builds running, commit
3735252d + this branch's working tree. 100 touched accounts (half existing, half
new), median of 5. "Full rebuild" is `sparse_accounts_root` over every account
merged with the overlay: what `root_with_overlay` did before.

```
accounts=    1000 touched=100: incremental median=1.4036ms (visited 1140 nodes) full-rebuild median=907.5µs seed+cache-build=16.8644ms
accounts=   10000 touched=100: incremental median=1.9786ms (visited 1736 nodes) full-rebuild median=9.2767ms seed+cache-build=58.4913ms
accounts=  100000 touched=100: incremental median=4.2204ms (visited 2328 nodes) full-rebuild median=97.7699ms seed+cache-build=500.521ms
accounts= 1000000 touched=100: incremental median=5.9081ms (visited 3087 nodes) full-rebuild median=1.108967s seed+cache-build=5.086373s
```

Per block, the incremental cost grows with depth (about log2 N levels per
touched key), not with N: 1.4 ms to 5.9 ms over a 1000× growth in accounts,
against 0.9 ms to 1.1 s for the full rebuild. At 1,000 accounts the cache is
**slower** than the full rebuild (RocksDB point reads cost more than hashing
1,000 leaves in memory). The one-time rebuild (activation, or after an
out-of-band write) is the `seed+cache-build` column's order: about 5 s at 1M.

Not measured: release profile; the cost the cache adds to each commit (the
delta is computed a second time at commit, by design, rather than carried from
the preview); `verify_root_cache` at chain open on a large state.

## Other runs

```
cargo test -p custom-l1-node --lib
test result: ok. 547 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 14.85s

cargo test -p custom-l1-node --test stateless_equivalence --test restart_tests --test state_layer_decode_tests
restart_tests:            test result: ok. 4 passed; 0 failed; 0 ignored
state_layer_decode_tests: test result: ok. 4 passed; 0 failed; 0 ignored
stateless_equivalence:    test result: ok. 11 passed; 0 failed; 0 ignored

cargo xtask spec-coverage --strict
62 rules, 164 vectors; 59 consensus rules covered, 0 gaps   (exit 0)
```

NOT VERIFIED locally: the other `custom-l1-node` integration targets, the
light-client crate, `cargo xtask coverage --verify-targets` (left to CI per
AGENT-RULES), release-profile timings.

## Revision after the PR #540 review (same day)

Branch `feat/incremental-state-root`, code commit 5bb1105e on top of 151628c4,
then docs. Everything below ran through `cargo-slot.sh` on this machine
(Windows 11 Pro 10.0.29680, shared with other builds), test profile.

### What changed

- **Branches only.** The first layout stored every node holding two or more
  accounts, single-child levels included, so recipient pairs one bit apart
  cost about 240 entries each (review, HIGH; found by reading the code, not
  run on the old layout). Now one entry per branch: `smt:` ‖ key bits padded
  to 32 bytes ‖ depth, value the two children's digests. N accounts, N − 1
  entries, whatever the addresses. Single-child runs are folded in memory.
  Code: `crates/node/src/state/root_cache.rs`, `root_cache/walk.rs`.
- **One walk a block.** The plan made with the root rides in the overlay and is
  staged at commit if the write generation it was made at is still current
  (`PreviewCache::begin_write_from`). The commit holds the write guard while it
  stages, and the marker is read from the walk's own snapshot.
- **Crossover.** A block with touched × (bits(N) + 1) × 2 > N (N ≥ 4,096)
  builds from every account instead.
- **Safety nets.** `check_declared_root`: a declared root that the full rebuild
  matches and the cache does not withdraws the cache and accepts the block.
  Debug builds cross-check every planned root against the full rebuild.
  `write_batch` treats any operation its iterator cannot report
  (`delete_range`, `merge`) as an account write. Repairs are counted
  (`StateDB::root_cache_stats`) and written to stderr.
- **Tests added:** `neighbour_addresses_cost_one_branch_each_not_a_spine`,
  `a_wrong_cache_costs_a_rebuild_not_the_block`,
  `every_write_that_can_touch_accounts_withdraws_the_cache`, a revert case in
  the scaling test, stats assertions in the property test,
  `a_snapshot_carries_no_root_cache_and_the_importer_builds_its_own` and
  `purging_a_failed_bootstrap_clears_the_root_cache` (`state_pruner::snapshot`),
  and `crates/node/tests/root_cache_chain_tests.rs` (the sparse tree activated by
  a genesis schedule entry; signed blocks; a reorg that reverts 3 blocks and
  applies 5; a restart; a damaged cache caught by `Chain::open`).

### Red (mutations, then reverted)

Four mutations at once: the walk returns Stale on any deletion; the
declared-root net never applies; `Chain::open` skips `verify_root_cache`;
`touches_accounts` ignores operations it cannot see.

```
test state::root_cache::tests::every_write_that_can_touch_accounts_withdraws_the_cache ... FAILED
test state::root_cache::tests::a_wrong_cache_costs_a_rebuild_not_the_block ... FAILED
test state::root_cache::tests::incremental_root_equals_full_rebuild_over_random_histories ... FAILED
test state::root_cache::tests::work_per_block_follows_touched_keys_not_total_accounts ... FAILED
assertion `left == right` failed: seed 0 step 12
  left: RootCacheStats { walked: 11, built: 2, repaired: 1 }
 right: RootCacheStats { walked: 12, built: 1, repaired: 0 }
test result: FAILED. 6 passed; 4 failed; 1 ignored; 0 measured; 543 filtered out; finished in 6.93s

test chain_open_rebuilds_a_damaged_cache_before_the_first_block ... FAILED
test blocks_a_reorg_and_a_restart_keep_the_cache_equal_to_the_full_rebuild ... FAILED
assertion `left == right` failed: Chain::open caught it
  left: 0
 right: 1
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.31s
```

The neighbour-pair test was not run against the old layout (its API differs).
Its red is the review's arithmetic; the test pins the new bound.

### Green

```
cargo test -p custom-l1-node --lib
test result: ok. 552 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 11.80s

cargo test -p custom-l1-node --test root_cache_chain_tests --test stateless_equivalence --test restart_tests --test state_layer_decode_tests --test pruned_node_tests --test consensus_tests
consensus_tests:          test result: ok. 31 passed; 0 failed; 0 ignored
pruned_node_tests:        test result: ok. 19 passed; 0 failed; 0 ignored
restart_tests:            test result: ok. 4 passed; 0 failed; 0 ignored
root_cache_chain_tests:   test result: ok. 2 passed; 0 failed; 0 ignored
state_layer_decode_tests: test result: ok. 4 passed; 0 failed; 0 ignored
stateless_equivalence:    test result: ok. 11 passed; 0 failed; 0 ignored

cargo xtask spec-coverage --strict
62 rules, 164 vectors; 59 consensus rules covered, 0 gaps
```

Red team: a full block (5 MiB / 64 B = 81,920 outputs) of `(x, x ^ 1)` pairs
on 1,000 accounts.

```
red team: 81920 neighbour accounts on 1000: 82907 cache writes, 8622387 batch bytes (105 per account), 82919 stored for 82920 accounts, walk+stage 2.7220998s
```

### Measured

`cargo test -p custom-l1-node --lib -- root_cost_by_state_size --ignored --nocapture`.
100 touched accounts (half existing, half new), median of 5, except
`plan+stage+write` (one run, on a fresh checkpoint copy) and `verify-at-open`
(one run). `plan+stage+write` is the accounts layer's whole per-block cost on
the cache path. "Full rebuild" is what `root_with_overlay` did before the cache.

```
accounts=    1000 touched=100: walk median=601.3µs (523 seeks) plan+stage+write=1.5468ms full-rebuild median=978.3µs verify-at-open=2.2461ms seed+cache-build=19.2824ms
accounts=   10000 touched=100: walk median=1.355ms (875 seeks) plan+stage+write=3.0466ms full-rebuild median=9.1311ms verify-at-open=22.3663ms seed+cache-build=42.1543ms
accounts=  100000 touched=100: walk median=1.4115ms (1144 seeks) plan+stage+write=4.7543ms full-rebuild median=105.539ms verify-at-open=232.7159ms seed+cache-build=348.8442ms
accounts=  100000 max block (81920 neighbour outputs): walk=1.4373122s rebuild=1.2925386s (the plan takes the rebuild: true)
accounts= 1000000 touched=100: walk median=2.4541ms (1520 seeks) plan+stage+write=9.6712ms full-rebuild median=1.0501912s verify-at-open=3.8583441s seed+cache-build=4.1345454s
accounts= 1000000 max block (81920 neighbour outputs): walk=1.6962205s rebuild=2.3255606s (the plan takes the rebuild: true)
```

Reading it:

- On the cache path, the accounts layer's per-block cost follows touched keys
  and log2 N: 1.5 ms to 9.7 ms from 1k to 1M accounts, against 0.98 ms to
  1.05 s for the full rebuild. At 1k accounts the cache is still slower.
- A maximal block of new neighbour accounts costs 1.3 to 2.3 s either way. At
  1M the walk was faster (1.70 s against 2.33 s), but the crossover picked the
  rebuild. The factor 2 leans to the rebuild on purpose, because its cost does
  not depend on the addresses chosen. For such a block the rebuild is O(N):
  this is the bound, not a per-block saving.
- `verify_root_cache` at open: 3.9 s at 1M accounts, one run, beside
  `check_tip_state`'s own full `state_root()`. O(N) memory for the account list.

An earlier run of the same test measured `plan+stage+write` through
`root_with_overlay` (1.23 s at 1M). That figure included the debug-build
cross-check, which is itself a full rebuild, so it was discarded and the test
now calls `plan` directly.

### Still not done, and why

- **Dark at genesis.** `STATELESS_ACTIVATION_HEIGHT = u64::MAX`. The genesis
  chain's accounts root is the dense tree and costs O(N) per block as before,
  plus one marker delete. Whether to activate at genesis is Eric's decision
  (ADR-062a "What it does not do" 1; DECISIONS.md 2026-11-14 revision).
- **Record layers** are still full rebuilds every block.
- **Release-profile timings:** not run.
- **A producer with a wrong cache** in a release build accepts its own block
  and forks off; `check_tip_state` then refuses to open the database on
  restart. There is no periodic deep check. *(Second revision: a periodic
  check now runs every 256 heights on the preview path; see below.)*
- `cargo xtask coverage --verify-targets` and workspace clippy: left to CI.

## Second revision after the PR #540 re-review (same day)

Changed: no crossover (a valid cache is always walked); the write generation
moves on entry to and exit from every write (`preview::WriteGuard`); a
`holds()` check on the preview path once per height that is a multiple of
`CONFIRM_EVERY` (256); debug builds outside unit tests repair instead of
panicking; checked add on the on-disk account count; invariant 24 amended;
the cost asymmetry for invalid blocks written into ADR-062a. Not changed: the
root value, the genesis format, Eric's (a)/(b) decision (still open).

### Red (two mutations, then reverted)

The `Drop` bump in `WriteGuard` commented out, and the first revision's
crossover put back in `plan`:

```
test state::root_cache::tests::a_plan_read_while_a_write_is_landing_is_not_staged ... FAILED
test state::root_cache::tests::a_busy_block_walks_and_writes_its_paths_not_the_whole_cache ... FAILED
---- state::root_cache::tests::a_plan_read_while_a_write_is_landing_is_not_staged stdout ----
panicked at crates\node\src\state\root_cache_tests.rs:70:5:
assertion `left == right` failed   (marker claims 10 accounts, state holds 11)
---- state::root_cache::tests::a_busy_block_walks_and_writes_its_paths_not_the_whole_cache stdout ----
panicked at crates\node\src\state\root_cache_tests.rs:544:5:
a valid cache is walked, whatever the block
test result: FAILED. 12 passed; 2 failed; 1 ignored; 0 measured; 543 filtered out; finished in 6.76s
```

### Green

```
cargo test -p custom-l1-node --lib -- root_cache state::preview
test result: ok. 17 passed; 0 failed; 1 ignored; 0 measured; 540 filtered out; finished in 6.20s

cargo test -p custom-l1-node --test root_cache_chain_tests --test stateless_equivalence \
  --test restart_tests --test pruned_node_tests --test consensus_tests --test consensus_conformance
consensus_conformance  test result: ok. 1 passed; 0 failed
consensus_tests        test result: ok. 31 passed; 0 failed
pruned_node_tests      test result: ok. 19 passed; 0 failed
restart_tests          test result: ok. 4 passed; 0 failed
root_cache_chain_tests test result: ok. 2 passed; 0 failed
stateless_equivalence  test result: ok. 11 passed; 0 failed

cargo xtask spec-coverage --strict
62 rules, 164 vectors; 59 consensus rules covered, 0 gaps:
```

`cargo clippy -p custom-l1-node --lib --tests`: no warning in
`state/root_cache*` or `state/preview.rs`; the two in `state/db.rs` (lines 177
and 929, `storage_err` and `write_overlay`) are on master's code.

### Measured (test profile, this PC, one run each, at the branch head)

`cargo test -p custom-l1-node --lib root_cost_by_state_size -- --ignored --nocapture`

```
accounts=    1000 touched=100: walk median=590.5µs (523 seeks) plan+stage+write=1.5909ms full-rebuild median=962.6µs verify-at-open=2.1444ms seed+cache-build=42.5503ms
accounts=   10000 touched=100: walk median=1.0309ms (875 seeks) plan+stage+write=2.9275ms full-rebuild median=9.2222ms verify-at-open=21.7044ms seed+cache-build=45.1331ms
accounts=  100000 touched=100: walk median=1.6089ms (1144 seeks) plan+stage+write=4.9257ms full-rebuild median=112.0122ms verify-at-open=259.5042ms seed+cache-build=436.4529ms
accounts=  100000 max block (81920 neighbour outputs): walk=1.5570192s rebuild=1.4174393s cache-writes=140124 batch-bytes=14572955
accounts= 1000000 touched=100: walk median=3.1318ms (1520 seeks) plan+stage+write=6.8865ms full-rebuild median=1.234321s verify-at-open=4.5281202s seed+cache-build=4.8006468s
accounts= 1000000 max block (81920 neighbour outputs): walk=1.9371057s rebuild=2.4825714s cache-writes=251275 batch-bytes=26132659
```

- A maximal block now always walks: 140,124 cache writes (14.6 MB of batch)
  at 100k accounts and 251,275 (26.1 MB) at 1M. The crossover's build would
  have staged all N − 1 branches plus a range tombstone (the review's
  estimate: about 105 MB at 1M; not re-measured, the path is gone).
- At 100k the walk of that block was 1.10× the rebuild (1.56 s against
  1.42 s); at 1M it was 0.78× (1.94 s against 2.48 s).
- The 1M `verify-at-open` was 4.53 s this run (3.9 s in the first revision's
  run): one sample each on a shared machine.
- Release profile: NOT MEASURED.
