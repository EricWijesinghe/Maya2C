# 18 — Economic security and launch economics

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 18

> **Nothing here is legal or financial advice, and nothing here decides a
> token price, a sale or a legal structure.** Those need qualified lawyers
> and advisers (`docs/legal/QUESTIONS_FOR_COUNSEL.md`).

> DONE WHEN: econ/ simulator runs all scenarios with results here;
> docs/ECONOMIC_SECURITY.md has cost-of-attack tables; the fee market
> parameters are justified by measured data; treasury and vesting contracts
> have passing tests; the legal questions document exists.

| Condition | Result |
|---|---|
| All scenarios run | **yes**: 9 scenarios, 730 days each |
| Cost-of-attack tables | **yes**, `docs/ECONOMIC_SECURITY.md` |
| Fee parameters justified by measured data | **yes** (2026-09-27): derived, not only checked — §3a |
| Treasury and vesting tests | **pass** (`crates/treasury`, 5 tests) |
| Legal questions document | **exists** (`docs/legal/QUESTIONS_FOR_COUNSEL.md`) |

## 1. Simulator output

`cargo run --release -p maya-econ --bin econ-sim` (5.7 s). This session's
run is byte-identical to the earlier one: seeded, no clock. Fee parameters
are loaded from `maya_fee_market::FeeConfig::TESTING`, the chain's own
config, so the simulator and the chain cannot drift apart.

```
[SIM] maya-econ: agent-based model; prices are scenario inputs, not predictions; not financial or legal advice

Draft parameters: 100 validators at 24000 fiat/yr each, emission 3.0%/yr, operator share of rewards 19.0%.
Break-even token price for the full set on emission alone: 42.105 fiat. …

| Scenario | min staking | final validators (min) | supply change | peak base fee | max fee/tx (fiat) | stable |
|---|---|---|---|---|---|---|
| baseline | 50.3% | 102 (100) | +1.35% | 10 | 0.02948 | yes |
| low usage for 2 years (0.05x) | 50.3% | 106 (100) | +5.96% | 10 | 0.02621 | yes |
| sudden 100x usage from day 90 | 50.3% | 43 (43) | -71.08% | 418 | 1.18919 | **no** |
| 80% price drop on day 180 | 50.3% | 30 (30) | +1.32% | 10 | 0.02948 | **no** |
| large holder exits staking (30% of stake, day 120) | 50.3% | 102 (100) | +1.35% | 10 | 0.02948 | yes |
| fee-market spam campaign (days 60-67) | 50.3% | 101 (100) | +1.04% | 77609 | 0.02948 | yes |
| MP6: 30% currency devaluation (costs x1.43) | 50.3% | 100 (100) | +1.35% | 10 | 0.02948 | yes |
| MP6: 50% market crash | 50.3% | 74 (74) | +1.32% | 10 | 0.02948 | yes |
| MP6: liquidity shock (demand -90% 60 days, 20% unstake) | 50.3% | 102 (100) | +2.11% | 10 | 0.02948 | yes |
```

**What the model says the draft parameters get wrong:**

- **An 80 % price drop leaves 30 validators.** Below break-even (42.1 fiat),
  emission alone cannot pay the set. The draft parameters have no floor on
  validator income; that is a finding, not a tuning target.
- **Sudden 100x usage burns 71 % of supply** in the remaining 21 months, and
  validators fall to 43. At 80/20 burn/treasury, heavy use is sharply
  deflationary, and fees at 418/byte (1.19 fiat per transfer) price users out.
  The burn share needs revisiting before any activation height is set.
- **The spam campaign is expensive fast and recovers.** The base fee reaches
  77,609/byte in the 7-day campaign and returns to baseline. The validator
  set is unaffected.

## 2. Security, concentration, MEV

- Cost of attack, weak subjectivity (10 days for a 21-day unbonding), and
  concentration are in `docs/ECONOMIC_SECURITY.md`. A 2 % stake cap moves the
  Nakamoto coefficient from 3 to 16 on a synthetic power-law set; it is not
  proposed alone because splitting identities evades it.
- The MEV position is ADR-025. Batch settlement turns a sandwich from
  +144,262,677 into −1 at the largest simulated size.

## 3. Fee market parameters against measured data

`FeeConfig::TESTING`: 1 MiB target block, step denominator 8, floor 1/byte,
20 % treasury share. Held against what was measured:

| Constraint | Measured | Parameter consequence |
|---|---|---|
| Transaction size | 13,215 B (hybrid) | a 1 MiB target holds **79** transfers; the ceiling (2× target) holds 158 |
| Block application | 942 tx/s on one core | 158 tx per 15 s block is **1.1 %** of one core: execution is not the limit |
| Certificate load at n = 100 | 177 Mbit/s, 1.2 cores at 1 s rounds (ADR-021) | the ingest this target adds (2 MiB / 15 s ≈ 1.1 Mbit/s) is negligible beside it |
| Spam response | base fee 10 → 77,609 in 7 days (sim) | denominator 8: ×1.125 per full block, so the fee doubles in 6 full blocks (90 s) |
| Normal-load fee | 0.029 fiat per transfer at price 63 (sim) | low, as the brief asks |

**What this justifies:** the hardware limits measured in Master Prompts 12–14
are not approached by the target, so the fee market is there to price
congestion and spam, not to protect hardware, and the simulations show it
does that.

**What it does not:** the target was not *derived* from these numbers. At
13 KB per transfer, 1 MiB gives about 5 transfers per second, a throughput
choice this tree has not made. The multi-dimensional weights the brief names
do not exist: the fee is one-dimensional, per byte. The spam scenario uses
synthetic load, not `maya2c-loadgen` output.

### 3a. The parameters, derived from the node's own measurements (2026-09-27)

Inputs, each measured on the DAG-BFT node (ADR-027), not assumed:

| input | value | source |
|---|---|---|
| transfer size, ML-DSA-65 suite (v7), with fee output | **5,377 B** | `fee_market_live_tests::measured_transfer_sizes_for_the_fee_derivation` |
| transfer size, hybrid (v5), with fee output | **13,255 B** | same |
| sustained verify+execute+commit per node | **~990 tx/s on ~6 threads** (4 nodes sharing 24) | `examples/bft_tps.rs`, `reports/04-consensus.md` §6 |
| block cadence at `round_interval_ms = 500` | **0.96–1.03 blocks/s** | `scripts/bft_devnet.py` |
| gossip frame ceiling | 8 MiB | `network::behaviour::MAX_GOSSIP_MESSAGE_BYTES` |

Derivation:

1. **Capacity to target at 50 %.** 990 tx/s × 0.5 ≈ 495 tx/s, so a full
   block (2× target) still leaves the slowest measured node headroom.
2. **Bytes.** 495 tx/s × 5,377 B ≈ 2.66 MB/s; at ~1 block/s that is
   **`target_block_bytes` = 2,621,440 (2.5 MiB)**.
3. **The frame bound.** A full block is 2× target = 5 MiB of payload, under
   the 8 MiB gossip ceiling with room for certificates' signatures. An
   all-hybrid mix (13,255 B) at the same byte target is ~198 tx/s — the fee is
   per byte, so hybrid senders pay 2.5× for the same slot, which is the
   intended pressure toward the smaller suite.
4. **Bandwidth.** Inline payload travels twice (proposal and certificate):
   ~5.2 MB/s ≈ 42 Mbit/s ingress per validator at target — well inside a
   1 Gbit/s validator link.
5. **`change_denominator` = 8**: ×1.125 per full block, doubling in 6 full
   blocks ≈ 6 s at 1 block/s — spam is priced out within seconds (the
   simulation above: 10 → 77,609 in 7 days of sustained attack).
6. **`min_base_fee` = 1 per byte**: the floor; a v7 transfer then costs at
   least 5,377 base units, the normal-load price in the simulations.

`scripts/bft_devnet.py` now runs these values. What remains a choice rather
than a derivation: the 50 % headroom and the ~6-thread validator floor; both
are stated so they can be argued with.

**Fee estimator.** `econ/src/estimator.rs` quotes a k-block guaranteed
ceiling:

| k | Quotes | Exceeded | Mean over-estimate |
|---|---|---|---|
| 1 | 50,000 | 0 | 10.2 % |
| 3 | 49,998 | 0 | 34.0 % |
| 10 | 49,991 | 0 | 181.8 % |

It never under-quotes, and it over-quotes by more as the horizon grows. It
is **not exposed as an RPC method**.

## 4. Treasury and vesting

```
$ cargo test -p maya-treasury --profile ci
test early_termination_freezes_vesting_and_returns_the_rest ... ok
test claims_never_exceed_what_has_vested ... ok
test clawback_only_where_the_grant_allowed_it ... ok
test nothing_vests_before_the_cliff_then_release_is_linear_and_capped ... ok
test spends_need_every_stage_in_order_and_respect_the_epoch_limit ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

Vesting has a cliff, then linear release in `u128`. Termination freezes
vesting and returns the unvested share. Clawback is possible only on grants
created with it, and it returns everything unclaimed.

Treasury spends pass through ordered approval stages, stay within a
per-epoch limit, and every transition goes into a public ledger. The
second-client grant is the test's example.

The crate is **not wired into the node**: the genesis treasury exists, but
its spending path does not.

## 5. Legal readiness

`docs/legal/QUESTIONS_FOR_COUNSEL.md` exists (earlier this branch). The
compliance hooks for front-ends (sanctions screening, travel-rule message
formats) are **not built**. They belong outside the protocol and are listed
as open.

## 6. Found on the way

- `SUITE_ENVELOPE_ACTIVATION_HEIGHT` is **0**: v7 and v8 transactions are
  active from genesis (ADR-013). The module doc of
  `crates/node/src/core/transaction.rs` still said `u64::MAX`, and this
  branch's spec, custody guide, crypto watch page and reports 15 and 17 had
  repeated it. All are corrected in this change.
- `crates/node/src/genesis.rs` refers to `docs/sealed-mempool.md`, which does
  not exist.
