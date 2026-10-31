# ADR-068 step 4: amount x amount product audit (2026-10-31)

Branch `fix/amount-product-overflow` off master. Scope: consensus paths in
`crates/ledger-math`, `dex`, `governance`, `fee-market`, `crates/node/src/state`
(mining, staking, fees, vault, dex, oracle), `eo-oracle`, `staking`, `amount`,
`node/src/consensus/uint.rs`, `node/src/genesis.rs`.

Method: `rg` for every `checked_mul`, `wrapping_mul`, `saturating_mul`,
`overflowing_mul`, `mul_div*`, `mul_wide`, `full_mul`, `u128::from(..) *`, and bare
`*` (spaced and unspaced) in those trees, excluding tests. Each hit read in context.

## Finding

**F1 (fixed): vault fee allowance saturated, letting fee outputs escape the limit.**
`crates/node/src/state/vault.rs`, `vault_outflow`:
`fees.required(size)` saturates to `Amount::MAX` when `base_fee * size` overflows.
The allowance was `to_u256(required).saturating_mul(4)`, which gives `U256::MAX`.
Then `sum(true).saturating_sub(allowed_fee)` is 0, so every collector output is
treated as fee and is not counted against the vault's instant limit.
Reachable: the fee market is designed to rise toward the 256-bit ceiling
(`crates/fee-market/src/base_fee.rs:154`, `rule.rs:278`), so a base fee near 2^254
is reachable over time.

Fix: `fee_allowance(required) -> Result<U256>` uses `checked_mul` and returns
`NodeError::Vault("fee allowance overflows 256 bits")`. Same result for every
in-range input; only the overflow case changes (now refused).

Test: `state::vault::tests::a_saturated_fee_requirement_cannot_widen_the_fee_allowance`
covers `Amount::MAX` (must be Err), `5` (= 20), `u128::MAX` (exact), and the 2^254
boundary (2^254 refused, 2^254 - 1 accepted).

Red-team: with the old saturating body restored, the test fails:
```
thread 'state::vault::tests::a_saturated_fee_requirement_cannot_widen_the_fee_allowance' panicked at crates\node\src\state\vault.rs:554:9:
assertion failed: fee_allowance(max).is_err()
test result: FAILED. 0 passed; 1 failed; ... exit=101
```
With the checked body: `test result: ok. 1 passed; 0 failed` (exit 0).

**Contradiction:** `DECISIONS.md` 2026-10-28 ("ADR-068 step 4: 512-bit product audit,
clean") accepted "a checked/saturating op that refuses or clamps" as safe. That missed
F1: here saturation is not a clamp of the value, it removes the limit. That entry should
be amended, not left as "clean".

## Sites (every consensus amount product in scope)

### Fixed
| # | Site | Product | Result |
|---|------|---------|--------|
| 1 | `node/src/state/vault.rs` `vault_outflow` | fee x 4 headroom (U256 saturating_mul) | F1, now checked |

### Proven safe: 512-bit or widening
| # | Site | Product | Reason |
|---|------|---------|--------|
| 2 | `ledger-math/src/lib.rs:197,227` | total x weight | `mul_wide` / `mul_div` (512-bit) |
| 3 | `amount/src/lib.rs:163-191` | general amount product | `mul_wide` builds 512-bit `Wide`; `checked_mul` is `to_amount()` of it; Kani proofs in `amount/src/proofs.rs` |
| 4 | `dex/src/math.rs`, `amm.rs`, `batch.rs`, `book.rs`, `matching.rs` | all x*y/z, k = x*y | `mul_div_floor/ceil` (512-bit), `Wide` for k; `None` -> `DexError::Overflow`; proofs in `dex/src/proofs.rs` |
| 5 | `governance/src/tally.rs:203,217` | turnout / quorum bps | `mul_wide` on both sides of a comparison, no overflow |
| 6 | `fee-market/src/base_fee.rs:56` | base x gap / scale | `mul_div` (512-bit) |
| 7 | `fee-market/src/split.rs:54` | fee x bps | `mul_div` |
| 8 | `fee-market/src/rule.rs:83` | linear x gain | `mul_div`; `unwrap_or(MAX)` is a documented clamp, then bounded against `linear` |
| 9 | `node/src/state/mining.rs:433` `bps_of` | reward x bps | `mul_div` (512-bit) |
| 10 | `node/src/genesis.rs:748` | balance x 10 000 | `mul_div`; comment says why |
| 11 | `staking/src/params.rs:73`, `staking/src/epoch.rs:316` | pool x part | `mul_div` |
| 12 | `staking/src/concentration.rs` | stake x BPS, sum x den, n x total | Not consensus (published). Stakes shifted to 64 bits, committee <= u16::MAX, so every product fits u128/i128 |

### Proven safe: checked, refuses on overflow
| # | Site | Product | Reason |
|---|------|---------|--------|
| 13 | `node/src/state/mining.rs:238` `lifetime_emission` | reward x epochs | `checked_mul` -> `None` -> genesis refuses |
| 14 | `node/src/state/mining_exec.rs:33` `dag_epoch` | epoch x epoch_blocks | `checked_mul` -> `NodeError::Mining` |
| 15 | `fee-market/src/execute.rs:179` | base_fee x size | `checked_mul` -> MAX -> `Underpriced` unless `max_fee` is MAX, then balance check refuses |
| 16 | `node/src/state/fees.rs:93` `required` | base_fee x size | `checked_mul` -> MAX, documented: "nobody can pay that". F1 was the caller that multiplied MAX again |

### Proven safe: fits its width by bound
| # | Site | Product | Reason |
|---|------|---------|--------|
| 17 | `fee-market/src/base_fee.rs:54`, `rule.rs:75` | target x denominator | u64 x u64 in u128 |
| 18 | `node/src/state/mining.rs:305` `demand` | shares x epoch_blocks | u64 x u64 in u128, then `try_from` |
| 19 | `staking/src/epoch.rs:106` | authored x BPS; threshold x expected | u64 x u16-scale and u16 x u64 in u128 |
| 20 | `staking/src/epoch.rs:130` | authored x expected | u64 x u64 in u128 |
| 21 | `node/src/consensus/uint.rs:152` `mul_u64_wide` | limb x multiplier + carry | <= (2^64-1)^2 + (2^64-1) = 2^128 - 2^64 in u128; `mul_div_u64` returns `None` past 256 bits (line 167) |
| 22 | `eo-oracle/src/lib.rs:66` ndvi | (nir - red) x BPS | i32. red, nir in [0, 64535] after BOA offset -1000, so product <= 6.45e8 < 2^31 |
| 23 | `fee-market/src/model/mod.rs` `gain` | i16 weight x i64 hidden | Not an amount. Q16 features, i16 weights, 6 inputs; `extreme` tests at `model/mod.rs:172-225` cover the range limits |

### Saturating, not a product of two amounts (accepted)
| # | Site | Reason |
|---|------|---------|
| 24 | `node/src/state/mining.rs:280` `target_shares.saturating_mul(4)` | u64 retarget clamp, then clamped again |
| 25 | `node/src/state/mining.rs:287` `retarget` `mul_div_u64` | Hash target x shares / observed (not an amount). Overflow intentionally saturates to the easiest limit, commented in source |

### Out of scope (observed, not consensus, not changed)
- `bins/htlc-watcher/src/submit.rs:78-79`: `U256 saturating_mul(size) .saturating_mul(2)`, off-chain fee estimate. Could under-quote a fee. Not consensus.
- `bins/pool-service/src/config.rs:267`, `pplns.rs:79,121`: `full_mul` / `mul_div_u64`, 512-bit. Fine.
- `crates/confidential-ai/src/protocol.rs:95`: `checked_mul`. Outside scope.
- `bins/l1-wallet/src/main.rs:529`, `bins/genesis-ceremony/src/main.rs:482`: `checked_mul`, tooling.

## Open observation (not changed)
- `fee-market` base fee is designed to rise to the 256-bit ceiling. Fee saturation to
  `Amount::MAX` then makes fees unpayable (liveness for users), by design. Not an
  overflow, not fixed here. Worth a policy decision (an explicit cap) before mainnet.

## Test output
- `cargo test -p custom-l1-node --lib fee_allowance` (checked): `test result: ok. 1 passed; 0 failed`, exit 0.
- Red-team (saturating body): `FAILED. 0 passed; 1 failed`, exit 101 (see above).
- `cargo test -p custom-l1-node --lib vault`: `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 451 filtered out`, exit 0.

NOT VERIFIED: `dex`, `fee-market`, `staking`, `ledger-math` and `governance` tests and Kani
proofs were not run in this session; those sites were audited by reading only. Build was
scoped to `custom-l1-node` with `CARGO_BUILD_JOBS=1`.
