# 2026-11-14 — Incremental sparse accounts root (ADR-062a)

Brief: "incremental root now" (Eric 2026-11-14 MCQ), re-freeze bundle 2. Branch
`feat/incremental-state-root` from `origin/master` 3735252d.

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
