---
title: Quickstart — a local chain and a transaction
description: Build the node and wallet, start a one-validator chain on your machine, and send a post-quantum-signed transfer.
---

About fifteen minutes, most of it the first build. Everything runs on your
machine; no public network is involved.

:::note[There is no public testnet yet]
This guide runs a private chain that only you can see. A public testnet is
being prepared; until it is live, a local chain is the only way to use
Maya2C. Nothing here is mainnet, and no token is for sale.
:::

## 1. Build

You need Rust (`rustup`) and a C++ toolchain, because RocksDB is compiled in:
Visual Studio Build Tools on Windows, `build-essential` and `clang` on Linux,
Xcode command-line tools on macOS.

```bash
git clone https://github.com/EricWijesinghe/Maya2C
cd Maya2C
cargo build -p maya2c-node -p l1-wallet
```

The binaries land in `target/debug/`. The commands below call them by name,
so add that directory to your `PATH`, or prefix each command with it.

## 2. A validator key and a wallet

A validator signs its votes with ML-DSA-65. A wallet holds a **hybrid** key —
ML-DSA-65 *and* SLH-DSA — and every transaction carries both signatures.

```bash
mkdir my-chain && cd my-chain
export L1_WALLET_PASSWORD='choose-a-password'

maya2c-node --generate-validator-key validator.key   # prints the public key
l1-wallet --keystore wallet.key generate             # prints your address
```

On Windows PowerShell, set the password with
`$env:L1_WALLET_PASSWORD = 'choose-a-password'`.

The keystore is encrypted with that password. On Windows the wallet warns that
it could not restrict the file's permissions; keep the directory private.

## 3. A genesis file

The genesis names the validator set and who holds the starting balance. Put
the validator public key from step 2 in `validators`, and your wallet address
in `allocations` and `bonds`:

```json
{
  "chain_id": "maya-local",
  "timestamp": 1790000000,
  "difficulty_bits": 0,
  "pow_limit_bits": 0,
  "allocations": [{ "address": "<YOUR_ADDRESS>", "balance": 10000000 }],
  "bft": {
    "validators": ["<VALIDATOR_PUBLIC_KEY>"],
    "anchor_timeout_ms": 1000,
    "batch_size": 500,
    "fees": {
      "initial_base_fee": 1,
      "min_base_fee": 1,
      "target_block_bytes": 2621440,
      "change_denominator": 8
    },
    "staking": {
      "epoch_blocks": 20,
      "bonds": [{ "operator": "<YOUR_ADDRESS>", "bond": 100000 }]
    }
  }
}
```

Save it as `genesis.json`. The `bft` section is what makes this a DAG-BFT
chain: blocks are ordered and finalised by the validator set, which is the
consensus mode Maya2C launches with.

## 4. Start the node

```bash
maya2c-node --genesis genesis.json --data-dir data \
    --rpc-addr 127.0.0.1:8545 --validator-key validator.key
```

It logs `bft: built block …` about once a second. Leave it running and open
a second terminal in the same directory (set the password there too).

Keep `--rpc-addr` on `127.0.0.1`. The node's RPC has no authentication; the
public face of a real network is the API gateway, not this port.

## 5. Send a transaction

```bash
l1-wallet --keystore wallet.key balance
# balance: 10000000
# nonce:   0

l1-wallet --keystore wallet.key send \
    --to 7777777777777777777777777777777777777777777777777777777777777777 \
    --amount 500 --nonce 0
# status:   accepted into the mempool

l1-wallet --keystore wallet.key balance
# balance: 9972990
# nonce:   1
```

The balance fell by 27,010: 500 sent, and 26,510 in fees. Fees are charged
by size, and a hybrid-signed transaction is about 13 KB — see
[wallet integration](/guides/wallet/) for why.

Read the recipient's balance straight from the node:

```bash
curl -s -X POST -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"get_balance","params":["7777777777777777777777777777777777777777777777777777777777777777"]}' \
  http://127.0.0.1:8545
# {"jsonrpc":"2.0","id":1,"result":{"address":"7777…","balance":500,"nonce":0}}
```

## What was measured

These outputs are from a real run on 2026-09-29 (Windows 11, dev build,
commit `a6d066f`), not from a mock-up. The four-validator version of the same
flow, `python scripts/bft_devnet.py`, measured ~0.95 blocks/s, a transfer
visible on all five nodes in 2.1–3.2 s, and every node agreeing on the block at
the common height. Those are one laptop's numbers on localhost, not a
statement about a real network.

## Next

- [Post-quantum signatures in your application](/guides/signatures/)
- [Mining, validators and rewards](/guides/mining/)
- [API reference](/guides/api/) · [Wallet integration](/guides/wallet/)
