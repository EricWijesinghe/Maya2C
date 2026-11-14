---
title: Mining, validators and rewards
description: How GPU mining rewards work on Maya2C, why validators order the blocks, and what is built today.
---

:::caution[No profit promised]
Mining is not an investment. Nothing on this site says what a reward is worth,
or that mining will earn back the electricity it uses. Testnet coins have no
value, and mainnet is not live.
:::

Short version: **miners are paid in the coin; validators order the blocks.** Mining
on Maya2C is a reward lane, not the consensus
([ADR-044](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-044-mining-rewards-lane.md)).
A miner submits proof-of-work shares and is paid from a fixed reserve. It never
proposes, orders or finalizes a block, so no amount of hash power can rewrite
or stall the chain. That is what lets anyone mine without making the network
weaker. This page says exactly what works today, so nobody spends electricity
on a misunderstanding.

## Where things stand

| | Today |
|---|---|
| GPU mining for rewards | Built; pays from mainnet genesis, which has not happened yet. Each epoch the miners' part of the reward (70% by [ADR-044](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-044-mining-rewards-lane.md)'s default) is paid from a reserve allocated at genesis, never minted. A share is paid a fixed amount of that budget, the budget divided by the epoch's target share count, and pays nothing once the budget is spent. Unpaid budget stays in the reserve. The validators' part goes to the committee through the staking payout. The reward halves on a schedule and then stays at a tail floor, so mining does not end ([ADR-068](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-068-forever-mining.md)). No premine; genesis holds only the keyless mining reserve and the disclosed minimum validator bonds (7 x min-self-bond). A genesis without `bft.mining` charges fees and pays no reward, so the stopped testnet has no reward lane. |
| Maya2C Hub | Planned: the desktop miner app, with the GPU miner inside, for Windows, macOS and Linux. Built in source (ADR-073 phase 1): one miner per device (CPU or each GPU), a rig view with per-GPU temperature, power and load, a watchdog that restarts a crashed miner after 2 s, 4 s, 8 s and so on, stops at five exits in ten minutes, stops a GPU above its temperature ceiling (80 °C default, set in Settings) and restarts it 5 °C lower, and an alert log. Optional OS notifications, off by default (Settings), for a miner that gives up or a GPU stopped for heat: the device name only, at most one per kind every ten minutes. The pool is the listed public node only; other nodes wait for the miner to check each share's work itself (ADR-073 phase 2). Not yet run end to end against a mining chain. No Hub installer is published for any platform, macOS included: the latest published release (`node-v0.1.0-testnet.4`) has none, so there is no Hub to download yet. The release workflow has build jobs for all three, but a build job is not a published installer. Not code-signed yet, so Windows and macOS will warn "unknown publisher". A release Hub refuses every miner unless the miner's SHA-256 is pinned at build time, and the release job does not set that pin yet. |
| Pool service (Stratum V2) | Built, measured on one 24-core PC (2026-11-01, `perf` profile, not the release build): 4,272.7 accepted shares per second at saturation, against 1,727.3 with eight validator workers. 10,000 miners at one share a minute, connected at a paced rate: 10,000 open, none failed, about 12 KiB each. Unpaced, about a third of 10,000 connects were refused; the cause is not isolated. |
| DAG-BFT validators | Works: blocks, finality, transfers, fees and staking bonds at genesis. This is the consensus ([ADR-016](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-016-launch-scope.md)). |
| Joining the validator set after genesis | Built and tested: a registration with a bond enters the committee at the next staking epoch (`crates/node/tests/bft_staking_tests.rs`). |

## Mining a local proof-of-work chain

Useful for exercising the miner, the difficulty adjustment and the GPU
kernels; it pays nothing of value.

```bash
cargo build -p maya2c-node -p maya2c-genesis
maya2c-genesis --chain-id maya-pow-local --difficulty-bits 8 --out genesis.json
maya2c-node --genesis genesis.json --data-dir data \
    --rpc-addr 127.0.0.1:8546 --mine --threads 2
# mined height 2 nonce 12 tip 00677864d4f1f928
# mined height 3 nonce 6 tip c8c0ec10640cfe33
```

Measured on 2026-09-29: four blocks in twenty seconds at 8 difficulty bits on
two threads of a laptop CPU, dev build. Raise `--difficulty-bits` and it slows
accordingly.

What it does not do, and why:

- **Blocks are empty.** The built-in miner does not yet assemble transactions
  from the mempool (`bins/maya2c-node/src/main.rs`, `mining_loop`). Send a
  transaction to a proof-of-work devnet and it waits in the mempool.
- **No reward.** There is no coinbase transaction and the header names no
  miner. Hash power is recorded only through an optional claim transaction
  that feeds governance weight, not balances.
- **`--mine` is refused by a production build**, and a production node refuses a
  proof-of-work genesis. That is deliberate: proof of work is a devnet mode.

## Why validators and not miners

Proof of work gives probabilistic finality: a payment is "probably final" after
some number of blocks. DAG-BFT gives a block final in about a second on a
healthy network and never reorganises it, which is what an application or a
bank needs to treat a transfer as settled. The trade-off is that a validator
set has to exist and be trustworthy, which is what staking and slashing are for.

## What is open before the lane runs

The reward lane is part of mainnet genesis. It does not run on the stopped
testnet, and a chain runs it when its genesis carries `bft.mining`.

1. **Mainnet genesis.** The lane starts when the mainnet genesis is signed with
   `bft.mining` set, per [ADR-044](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-044-mining-rewards-lane.md).
2. **Validators at launch.** A project-run validator set at launch, stated
   publicly. Separate operators join after ([ADR-047](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-047-single-pc-foundation-set.md)).
3. **ADR-044 is Proposed.** Its parameters are defaults the project owner may
   change until the launch freeze.

Testnet coins, when a testnet exists, will come from a faucet and will have no
value. Anyone offering to sell you Maya2C coins today is not selling anything
this project issued.
