# 26 — Scale without fragmentation

**Date:** 2026-09-26 · **Branch:** `claude/task-0g86kl` · **Brief:** Master Prompt 26
Hardware record: [12-baseline.md](12-baseline.md).

> DONE WHEN: multi-machine validator prototype is measured; local fee
> markets and reserved lanes pass tests; the viral-app scenario keeps other
> apps within SLO; reports/26-scale.md has the numbers with hardware records.

| Condition | Result |
|---|---|
| Multi-machine validator prototype, measured | **not built**; one machine only. The single-machine parallel curves are in `reports/12-performance.md` (3.56x on 4 cores, uniform accounts) |
| Local fee markets and reserved lanes pass tests | **pass** (`crates/lanes`) |
| Viral-app scenario keeps other apps within SLO | **yes, with a design rule this run found**, and a configuration that breaches it (below) |
| Payments-grade targets (sub-second soft confirmation, finality in seconds) | not measurable: no BFT finality (ADR-015) |

## The viral-app scenario

```
$ cargo test -p maya-lanes --profile ci --test viral_app -- --nocapture
[SIM] local + cap, global target 150 (below cap + demand): other apps' median fee 2 -> 152 (76.0x), p99 inclusion 0 blocks; 24800 viral txs included; 12450 txs dropped after 30 blocks
[SIM] local + cap, global target 160: other apps' median fee 2 -> 2 (1.0x), p99 inclusion 0 blocks; 24800 viral txs included; 12450 txs dropped after 30 blocks
[SIM] global fee only, target 160 (control): other apps' median fee 2 -> 739 (369.5x), p99 inclusion 0 blocks; 41300 viral txs included; 1890 txs dropped after 30 blocks
```

The model:

- 10 apps, 8 transactions each per block at baseline, and blocks of 200;
- from block 100, app 0 wants 135 per block, 60 % of demand;
- senders bid 2–5× their app's current price (viral users 2–20×) and give
  up after 30 blocks.

It is a model (`[SIM]`), with the transaction count as the unit of load.

**What it shows.**

- **A single global fee market makes everyone pay for the viral app:** the
  other nine apps' median fee rises 370×.
- **A per-app local fee alone is not enough.** The viral app's traffic fills
  blocks and lifts the global fee regardless. What isolates it is the local
  fee **plus a cap on one app's share of a block** (80 of 200 here), the idea
  Solana applies per account.
- **The design rule the simulation found.** The global target must exceed
  the per-app cap plus the rest of steady demand. At 150, below 80 + 72 = 152,
  EIP-1559's "rise by at least 1" step lifts the global fee every block, and
  the other apps' fee still rises 76×. At 160 it stays flat.
- **The cost moves, it does not vanish.** Under the cap, 12,450 of the viral
  app's transactions go unserved within 30 blocks, against 1,890 under a
  global market. The viral app's own users bear its congestion; that is the
  intended trade.
- **Inclusion latency is 0 blocks in every run,** because senders bid
  multiples of the price. The SLO is met on latency trivially. The fee column
  is where the configurations differ.

## Lanes

```
$ cargo test -p maya-lanes --profile ci
test the_auction_caps_each_holder_and_the_total ... ok
test lane_traffic_goes_first_and_unused_lane_space_is_released ... ok
test a_hot_app_raises_only_its_own_local_fee ... ok
```

- The auction is pay-as-bid, highest price first, capped per holder and in
  total, so no one buys the whole chain.
- Lane traffic is included first, even when it pays less than general
  traffic.
- A lane's unused slots return to the general pool in the same block.

## Not done

- A validator as a cluster (multi-machine execution with one state root).
- Atomic cross-shard bundles: sharding is not in launch scope (ADR-016).
- Wiring local fees and lanes into the node, whose fee market is inactive.
