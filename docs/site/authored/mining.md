---
title: Mining, validators and rewards
description: How GPU miners earn on Maya2C, why validators order the blocks, and what is built today.
---

Short version: **miners earn the coin; validators order the blocks.** Mining
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
| GPU mining for rewards | Built, and live from mainnet genesis. Each epoch the miners' part of the reward (70% by [ADR-044](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-044-mining-rewards-lane.md)'s default) is paid from a reserve allocated at genesis, never minted. A share earns a fixed amount of that budget, the budget divided by the epoch's target share count, and pays nothing once the budget is spent. Unpaid budget stays in the reserve. The validators' part goes to the committee through the staking payout. The reward halves on a schedule and then stays at a tail floor, so mining does not end ([ADR-068](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-068-forever-mining.md)). No premine. A genesis without `bft.mining` is fees-only, so the stopped testnet has no reward lane. |
| Maya2C Hub | Planned: the desktop miner app, with the GPU miner inside, for Windows, macOS and Linux. The release workflow builds installers for all three, but the latest published release (`node-v0.1.0-testnet.4`) has none, so there is no Hub to download yet. Not code-signed yet, so Windows and macOS will warn "unknown publisher". A release Hub refuses every miner unless the miner's SHA-256 is pinned at build time, and the release job does not set that pin yet. |
| Pool service (Stratum V2) | Built, measured on one 24-core PC (2026-11-01, `perf` profile, not the release build): 4,272.7 accepted shares per second at saturation, against 1,727.3 with eight validator workers. 10,000 miners at one share a minute, connected at a paced rate: 10,000 open, none failed, about 12 KiB each. Unpaced, about a third of 10,000 connects were refused; the cause is not isolated. |
| DAG-BFT validators | Works: blocks, finality, transfers, fees and staking bonds at genesis. This is the consensus ([ADR-016](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-016-launch-scope.md)). |
| Joining the validator set after genesis | Built and tested: a registration with a bond enters the committee at the next staking epoch (`crates/node/tests/bft_staking_tests.rs`). |

## Mining a local proof-of-work chain

Useful for exercising the miner, the difficulty adjustment and the GPU
kernels; not for earning anything.

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
