---
title: Mining, validators and rewards
description: What mining does on Maya2C today, why the launch network is run by validators, and what is not built yet.
---

Short version: **you can mine a local proof-of-work chain today, but it pays
nothing and it is not how the public network will run.** The launch network is
secured by validators, not miners. This page says exactly what works and what
does not, so nobody spends electricity on a misunderstanding.

## Where things stand

| | Today |
|---|---|
| Proof-of-work mining (ArgonBlake) | Works on a **local devnet**. Blocks are empty and there is no block reward. Disabled in the production build. |
| DAG-BFT validators | Works: blocks, finality, transfers, fees and staking bonds at genesis. This is the launch consensus ([ADR-016](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-016-launch-scope.md)). |
| Block reward / coin issuance | **None.** No coin is minted after genesis. The base fee is burned; validators are paid from transaction tips each staking epoch ([ADR-029](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-029-fee-market-live.md)). |
| Joining the validator set after genesis | Built and tested: a registration with a bond enters the committee at the next staking epoch (`crates/node/tests/bft_staking_tests.rs`). Not usable publicly until a public testnet exists. |

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
