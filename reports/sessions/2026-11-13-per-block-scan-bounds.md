# 2026-11-13: per-block work against stored records

Re-freeze bundle, branch `fix/per-block-scan-bounds` (base master 3735252d).
Follows PR #500, which bounded the governance settlement pass. Decision:
ADR-062 amendment 2026-11-13; DECISIONS.md, same date.

## Method

Every pass `stage_block` runs, plus the root, read for work that grows with
stored state or with records an attacker creates. Each candidate timed in
`crates/node/src/state/scan_bounds_tests.rs` (`#[ignore]`d timing tests), on
the `test` profile: node crate at opt-level 0, dependencies at opt-level 3.
Machine: Intel Core Ultra 9 275HX, Windows 11 Pro Insider Preview 29680,
shared with other builds. Median of 3. Debug-crate figures; release not
measured except where `reports/12-performance.md` already did.

```
cargo test -p custom-l1-node --lib scan_bounds_tests -- --ignored --nocapture --test-threads=1
```

## Before the fix (master code)

```
dex-settle touched_pairs=  1 orders_each= 1000: median=9.2989ms (9.298µs/order)
dex-settle touched_pairs=  1 orders_each= 4096: median=23.9101ms (5.837µs/order)
dex-settle touched_pairs= 16 orders_each= 4096: median=545.1994ms (8.319µs/order)
sealed-settle due=   128: median=624.6µs
sealed-settle due=  1000: median=15.6514ms
sealed-settle due= 10000: median=1.8215442s
sealed-settle due= 32640: median=18.9530796s
staking delegations=  1000 record=   96219 B: decode+encode=1.8223ms settle_staking=3.0405ms overlay_clone=5.2µs
staking delegations= 10000 record=  960219 B: decode+encode=15.3638ms settle_staking=29.749ms overlay_clone=26.5µs
staking delegations= 87000 record= 8352219 B: decode+encode=152.6915ms settle_staking=288.8307ms overlay_clone=3.263ms
staking count_slots slots=1000000: median=57.9546ms
state-root accounts=   1000: median=1.4985ms
state-root accounts=  10000: median=13.9218ms
state-root accounts= 100000: median=147.8281ms
state-root records =   1000 (200 B each): median=1.8712ms
state-root records =  10000 (200 B each): median=18.1289ms
state-root records = 100000 (200 B each): median=205.7625ms
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 544 filtered out; finished in 69.19s
```

## After the fix

The timing tests seed state directly, past admission, so they time the passes
at sizes the new caps no longer admit; the bound rows are the 128-envelope,
4-book and 10,000-delegation lines.

```
dex-settle touched_pairs=  1 orders_each= 1000: median=9.0442ms (9.044µs/order)
dex-settle touched_pairs=  1 orders_each= 4096: median=35.6636ms (8.706µs/order)
dex-settle touched_pairs=  4 orders_each= 4096: median=85.9672ms (5.247µs/order)
dex-settle touched_pairs= 16 orders_each= 4096: median=549.11ms (8.378µs/order)
sealed-settle due=   128: median=390.5µs
sealed-settle due=  1000: median=3.5459ms
sealed-settle due= 10000: median=53.8383ms
sealed-settle due= 32640: median=182.984ms
staking delegations=  1000 record=   96219 B: decode+encode=1.2113ms settle_staking=2.4212ms overlay_clone=4.6µs
staking delegations= 10000 record=  960219 B: decode+encode=15.3092ms settle_staking=28.3566ms overlay_clone=26.6µs
staking delegations= 87000 record= 8352219 B: decode+encode=152.2573ms settle_staking=250.1608ms overlay_clone=4.3953ms
staking count_slots slots=1000000: median=57.7922ms
state-root accounts=   1000: median=1.8361ms
state-root accounts=  10000: median=16.9861ms
state-root accounts= 100000: median=131.2939ms
state-root records =   1000 (200 B each): median=2.5096ms
state-root records =  10000 (200 B each): median=21.1617ms
state-root records = 100000 (200 B each): median=212.7158ms
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 543 filtered out; finished in 11.64s
```

The sealed rows above 128 fell from quadratic to linear because of the ranged
overlay read alone (`Overlay::records_under`); the cap is what bounds them.

## Attacker cost (computed from the compiled constants)

A minimal transaction is about 13.2 KB (13,201 B governance, 13,215 B
transfer). A 5 MiB block holds about 397; the testnet fee target (2.5 MiB)
about 198, below which the base fee does not rise. Floor: 1 base unit a byte,
10^24 base units a coin. Ceiling (`MAX_BASE_FEE_PER_BYTE`): about 0.1 coin per
minimal transaction, about 39.7 coins per full block.

| Site | Records needed | Blocks | Fees at floor | Fees at ceiling | Old cost of the block it hurts | Class before | Now |
|---|---|---|---|---|---|---|---|
| Sealed, one reveal height | 32,640 envelopes | 255 (128 a block, under target) | ~4e-16 coin | ~3,264 coins | 18.95 s | CRITICAL where a sealed committee is in genesis (optional per network, ADR-025) | 128 per height: 0.39 ms |
| DEX books | 190 pairs × 4,096 orders = 778,240 placements, escrow ≥ 1 quote unit each | ~3,930 at target | ~1e-14 coin | ~77,800 coins | 190 × 4,096 × ~8.4 µs ≈ 6.5 s a block (extrapolated from 16 books, 549 ms), sustained by 190 cheap touches a block | CRITICAL | 16,384 orders a block: 86 ms |
| Staking record | 87,000 delegations from distinct funded addresses, 10 base units each | ~440 at target | ~free | ~8,700 coins | 289 ms every block, plus 152 ms per staking tx (397 in a block ≈ 60 s) | CRITICAL | 1 MiB + 16 tx: ≈ 28 + 16 × 15 ≈ 270 ms worst (computed from the 10,000 row) |
| State root, accounts | 2.1 M accounts (≈ 1 s in release at 0.48 µs) | ~26 full blocks (~65,000 outputs per 4 MiB tx) | ~1e-15 coin | ~1,030 coins | ≈ 1 s per block, every block, and growing | CRITICAL | **not fixed** |

## Every site checked

| Site | Grows with | Bound | Class |
|---|---|---|---|
| `root_with_overlay` / `state_layers` (accounts, channels, every `RECORD_LAYERS` prefix, contracts, nullifiers) | all committed state | none | **CRITICAL, not fixed** (ADR-062a) |
| `settle_governance` | every proposal ever | due index, 64 per height (PR #500) | fixed in #500 |
| `settle_sealed` | envelopes at one height | 255 × 128 before; 128 now | CRITICAL, fixed |
| `gather_shares` / `merged_records` overlay part | whole block overlay per call (quadratic) | ranged read now | HIGH (in-block), fixed |
| `settle_trading` → `settle_book` → `load_order_records` | touched pairs × book size | 16,384 orders now | CRITICAL, fixed |
| `settle_batch` | swaps per pair | `MAX_BATCH_INTENTS` 256 | bounded |
| `match_book` fills | fills | `DexMaxFillsPerBlock` ≤ 4,096 | bounded |
| `settle_oracle` | nothing (one beacon record) | O(1) | bounded |
| `feeds_through` (per VM call) | feeds, authority-created only; overlay part ranged now | registry size | bounded |
| `settle_fees` | nothing | O(1) | bounded |
| `settle_mining` → `prune_seen` at an epoch boundary | shares in the epoch | `max_shares_per_block` × `epoch_blocks`, each share proof of work; not timed | MEDIUM, not timed |
| `settle_staking` / `apply_staking` | whole staking record | 8 MiB before; 1 MiB and 16 tx now | CRITICAL, fixed (interim) |
| `count_slots` | anchor-round gap | `MAX_SLOTS_PER_BLOCK` 1,000,000: 58 ms | MEDIUM, bounded |
| `end_epoch` (rewards, unbonding queue) | delegations and unbondings | inside the record cap | bounded by the cap |
| `check_invariants` (conservation, anomaly) | overlay only | block | bounded |
| `mark_sparse_accounts` | one key | O(1) | bounded |
| `effective_schedule_through` (`up:`) | upgrade entries | governance- and council-gated, versions and suites | bounded |
| `capture_undo` / `write_overlay` | overlay only | block | bounded |
| `select_applicable` trial `overlay.clone()` per non-transfer tx | overlay size | quadratic in the block, documented in `reports/12-performance.md`; 3.3–4.4 ms per clone with an 8 MiB staking record staged | MEDIUM, smaller with the 1 MiB cap |
| Shielded pool commitment | frontier tree + anchor window | fixed size | bounded |
| VM `call_contract` → `contract_storage` | whole storage of the called contract, per call | none; attacker-grown | **HIGH, not fixed, not timed** |
| VM `gas_limit` | sender-chosen, uncapped (`state/context.rs` says so) | none; 16.6 M fuel/ms (`docs/zkml.md`) means `u64::MAX` is decades of compute | **CRITICAL, not fixed**: not a stored-state scan, outside this change |
| Vault withdrawals, threat indicators, stored headers, archive receipts | per owner / RPC / startup | not on the apply path | n/a |
| Chain fork choice `load_records` / `adopt_heaviest` | headers | startup only | n/a |
| Post-genesis: names commit prune (#493), scheduled payments (#502) | due index | 1,024 and 256 per block | bounded; comments on the PRs |
| Post-genesis: paymaster (#501), token policy (#487), account authority (#447) | no end-of-block pass | — | n/a |

## Tests (real output)

```
cargo test -p custom-l1-node --lib scan_bounds_tests -- --include-ignored --nocapture --test-threads=1
test state::scan_bounds_tests::a_block_carries_at_most_its_staking_transactions ... ok
test state::scan_bounds_tests::a_reveal_height_holds_at_most_its_cap ... ok
test state::scan_bounds_tests::the_books_a_block_touches_stay_within_the_budget ... ok
test state::scan_bounds_tests::the_ranged_merge_matches_the_full_scan ... ok
test state::scan_bounds_tests::the_staking_record_stops_growing_at_its_cap ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 543 filtered out; finished in 11.64s
```

Red run, each guard disabled (`if false && …`, old 8 MiB cap), then restored:

```
test state::scan_bounds_tests::a_reveal_height_holds_at_most_its_cap ... FAILED
test state::scan_bounds_tests::a_block_carries_at_most_its_staking_transactions ... FAILED
test state::scan_bounds_tests::the_books_a_block_touches_stay_within_the_budget ... FAILED
test state::scan_bounds_tests::the_staking_record_stops_growing_at_its_cap ... FAILED
test state::scan_bounds_tests::the_ranged_merge_matches_the_full_scan ... ok
test result: FAILED. 1 passed; 4 failed; 5 ignored; 0 measured; 543 filtered out; finished in 0.91s
```

The equivalence test passes on both by design: it compares the old filter,
kept in the test file, with the ranged read on random keys over a four-byte
alphabet.

```
cargo test -p custom-l1-node --lib --test sealed_tests --test dex_tests --test dex_concurrency_tests \
  --test bft_staking_tests --test exploit_replays --test oracle_tests --test conformance --test block_bytes_tests
lib                    ok. 547 passed; 0 failed; 6 ignored
bft_staking_tests      ok. 8 passed
block_bytes_tests      ok. 10 passed
conformance            ok. 6 passed
dex_concurrency_tests  ok. 9 passed
dex_tests              ok. 36 passed
exploit_replays        ok. 11 passed
oracle_tests           ok. 30 passed
sealed_tests           ok. 20 passed

cargo clippy -p custom-l1-node --lib --profile test
warning: `custom-l1-node` (lib test) generated 110 warnings (74 duplicates)
```

None of the clippy warnings is on a line this change wrote (filtered on the
changed files: all pre-existing pedantic lints).

## NOT VERIFIED

- Release-profile timings for anything but the root (already in `reports/12-performance.md`).
- Mining `prune_seen` at an epoch boundary and VM per-call storage load: not timed.
- Workspace tests, the pedantic ratchet, `coverage --verify-targets`: left to CI.
- `cargo xtask spec-coverage`: not run; no vector exercises these passes, and no key layout changed.
