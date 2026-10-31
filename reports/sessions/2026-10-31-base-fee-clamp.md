# 2026-10-31 — base-fee step clamp and ceiling (ADR-071)

Branch `fix/base-fee-clamp` off origin/master. Proposed, not merged. ADR-071 (not ADR-069: that number is taken by `ADR-069-post-mainnet-feature-program.md`).

## Verified claims

| Claim | Result | Evidence |
|---|---|---|
| Rise has no per-block cap (fee ~8.9x per block at 64x target) | Verified | Red-team on unclamped rise: 20 blocks of 64x target from fee 1000 gives `9191016401140048149974` (bound after fix: 10,545). `fee 1 rose to 2305843009214` in one block. |
| `limits.rs` "1/8 per block" holds only for size <= 2x target | Verified | Step over 2x target is k/8 of the fee, k > 1. |
| `limits.rs` floor only; validate has no ceiling | Verified before change | `limits.rs` had only `MIN_BASE_FEE_FLOOR`. |
| Active rule is the linear one (`state/fees.rs` next) | Verified | `FeeRecord::next` calls `maya_fee_market::next_base_fee`. Neural rule not called by node. |
| Per-block byte cap | **Not found** | No block-size constant. Per-vertex caps only: `MAX_BATCH_BYTES` 4 MiB (`consensus/bft/store.rs:53`), `MAX_BATCH_ITEMS` 4096 (`:62`), `MAX_COMMITTEE` 100 (`:66`). Block = one committed sub-DAG (`consensus/bft/builder.rs`), `candidate_block_sealed` (`consensus/chain.rs:906`) has no size check. Reported, not worked around (ADR-071 Follow-up). |
| Standard transfer size | Recorded 13,215 B, not re-measured | `reports/13-pq-weight.md`. |
| Decimals 18 (ADR-046) | Superseded by code: 24 | `apps/wallet-gui/core/src/amount.rs:11`; merged #230. |

## Changes

- `crates/fee-market/src/limits.rs`: `MAX_BASE_FEE_PER_BYTE` placeholder, value = middle option (0.1 MAYA per 13,215 B at 24 decimals) = `7_567_158_531_971_244_797`. **PENDING ERIC.**
- `crates/fee-market/src/base_fee.rs`: rise `min(max(delta,1), max(parent/D,1))`; ends at `max(ceiling, parent)`.
- `crates/fee-market/src/config.rs`: `ConfigError::Ceiling` when `initial_base_fee > MAX`.
- `crates/fee-market/src/rule.rs`: neural rule gets the same share and ceiling (keeps unit-gain = linear). Two unit tests updated: a fee above the ceiling holds.
- `crates/fee-market/src/proofs.rs`: Kani harness `a_rise_is_one_denominator_share_and_stops_at_the_ceiling`.
- `crates/fee-market/tests/base_fee_clamp_tests.rs`: 6 new tests.
- `crates/node/tests/fee_market_tests.rs`: `a_max_fill_attack_cannot_price_out_a_one_coin_account` (uses real signed-transfer size).
- `crates/spec-ref/src/fees.rs`, `src/bin/spec-vectors.rs`, `spec/tests/fees.json` (regenerated): FEE-3 rise cap, FEE-6 ceiling vectors, `ceiling` field.
- `spec/04-fees.md`: FEE-3 rewritten, FEE-6 added.
- `spec/verifier-ts/verify.ts`: clamp and ceiling.
- `formal/z3/fee_market.py`: model + 2 new proofs.
- `docs/adr/ADR-071-base-fee-step-clamp-and-ceiling.md`: Proposed, options table.

## Real command output

Fee market, all targets (`CARGO_BUILD_JOBS=1 cargo test -p maya-fee-market`, after the final code change except a cfg(kani)-only edit):

```
Running unittests src\lib.rs     test result: ok. 47 passed; 0 failed
Running tests\base_fee_clamp_tests.rs   test result: ok. 6 passed; 0 failed
Running tests\lean_differential.rs      test result: ok. 1 passed; 0 failed
Running tests\mutation_boundaries.rs    test result: ok. 6 passed; 0 failed
Running tests\property_tests.rs         test result: ok. 13 passed; 0 failed
EXIT 0
```

Red-team: new `base_fee_clamp_tests` against the old unclamped rise (`base_fee.rs` rise branch reverted, ceiling still in validate; file restored and byte-compared afterwards):

```
the_ceiling_is_a_configuration_bound                          ok
the_ceiling_is_never_exceeded_under_sustained_full_blocks      FAILED  fee 10267985824349953545 above the ceiling
a_single_block_never_raises_the_fee_by_more_than_one_denominator_share  FAILED  fee 1 rose to 2305843009214
twenty_blocks_at_sixty_four_times_target_rise_no_faster...    FAILED  after 20 full blocks fee is 9191016401140048149974, bound 10545
a_one_coin_account_pays_a_standard_transfer_after_a_max_fill  FAILED  Option::unwrap() on None (fee x 13,215 overflows 256 bits)
a_fee_above_the_ceiling_holds_and_does_not_fall...            FAILED
test result: FAILED. 1 passed; 5 failed
```

Spec, `cargo test -p maya-spec-ref`: `test result: ok. 8 passed; 0 failed`, EXIT 0.
Vectors: `spec-vectors --check`: EXIT 0 (committed `fees.json` matches the reference).
Spec coverage, `cargo xtask spec-coverage`:

```
FEE-3    04-fees.md   5  0  covered (positive only)
FEE-6    04-fees.md   3  0  covered (positive only)
53 rules, 105 vectors; 50 consensus rules covered, 0 gaps:
EXIT 0
```

TypeScript verifier, `node --experimental-strip-types spec/verifier-ts/verify.ts`:

```
verifier-ts (independent, signatures trusted from vectors): {"state_transitions":20,"encoding":9,"fees":20,"headers":3,"mining":17,"keys":4} cases, 0 mismatches
```

rustfmt: `proofs.rs` parses and is formatted (`--check` rc 0); other edited files formatted.

## NOT VERIFIED

- **Node test** `crates/node/tests/fee_market_tests.rs::a_max_fill_attack_cannot_price_out_a_one_coin_account` is written but **not compiled or run**. The fresh worktree target needed a full `custom-l1-node` dependency build at `CARGO_BUILD_JOBS=1`; it reached 9 crates in about an hour and was stopped by PID (commit-charge limits). Run `cargo test -p custom-l1-node --test fee_market_tests` when a full build is affordable.
- **Kani** harness: Kani is WSL only; not run.
- **Z3** model: `z3` is not installed here (`ModuleNotFoundError: No module named 'z3'`); not run.
- **Clippy / lint ratchet** on the changed crates: not run.
- **Standard transfer size** 13,215 B: from `reports/13-pq-weight.md`, not re-measured.
- **features.toml / PROGRESS.md**: not updated; this is a fix to an existing fee-market entry, to be confirmed by reviewer.

## Options for the ceiling (Eric decides)

| Option | Cost of one standard transfer at cap | Per byte (base units, 24 dec) | Share of 1-coin balance |
|---|---|---|---|
| A | 0.01 MAYA | 756,715,853,197,124,479 | 1% |
| B (default, pending Eric) | 0.1 MAYA | 7,567,158,531,971,244,797 | 10% |
| C | 1 MAYA | 75,671,585,319,712,447,975 | 100% |

Recovery at full blocks from 10 to the B cap: about 354 blocks under the new rule, about 19 under the old.

## Other findings

- Another session holds `cargo test -p maya-wallet-core` (PID 16828). Not touched.
- Commit requested as `ADR-069`; this is ADR-071. Co-author line per the user's instruction (Opus 5.5).
