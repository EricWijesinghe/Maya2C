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
| GPU mining for rewards | Built: a miner's shares (`WorkClaim`) are paid **70% of each epoch's issuance**, split by share count; the other 30% goes to validators. Paid out of a reserve allocated at genesis, never minted, capped at 21,000,000; issuance ends when the reserve is empty. No premine. A tail reward funded to the last height (ADR-068, [PR #221](https://github.com/EricWijesinghe/Maya2C/pull/221)) is proposed and not on master yet. **Mainnet has not launched**, so mining earns nothing yet. |
| Maya2C Hub | The desktop miner app, with the GPU miner inside, for Windows, macOS and Linux. Installers are attached to each release. They are not code-signed yet, so Windows and macOS warn "unknown publisher". |
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

## What is needed before anyone can earn

These are open decisions, not tasks waiting for time:

1. **A decision on issuance.** Today validators earn only tips, and emission is
   nil. Whether that stays true, or new coins are minted for validators or a
   treasury, is an economic decision for the project owner.
2. **A public testnet** with seed validators, so outside validators have
   something to join and register against.
3. **At least four independent validators.** A DAG-BFT committee of *n*
   tolerates ⌊(*n*−1)/3⌋ faults: one validator halts if it stops, two halt if
   either stops, four survive one failure.

Testnet coins, when a testnet exists, will come from a faucet and will have no
value. Anyone offering to sell you Maya2C coins today is not selling anything
this project issued.
