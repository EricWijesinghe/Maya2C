---
title: Run a validator
description: How independent operators join maya-testnet-1 as validators, what it takes, and what is still in the way.
---

Maya2C finalizes blocks with DAG-BFT: a committee of validators signs every
round, and a block is final once a quorum has certified it. A network run
by one operator is a demo. A network run by many independent operators is
a blockchain. This page is how you become one of them.

:::caution[No money, no tokens]
Testnet coins have **no value**. Running a testnet validator earns public
recognition only: your name in the validator list and in the release
notes. Nothing on this site promises tokens, money or a mainnet
allocation, and anyone who says otherwise is not speaking for this project.
:::

## Where things stand

| | Status |
|---|---|
| Validator software | Shipped: `maya2c-node` in the [releases page](https://github.com/EricWijesinghe/Maya2C/releases) |
| Registering a key | Shipped: `l1-wallet register-validator` |
| Rejoining after an outage | Shipped: attested checkpoints, `--catch-up-from` ([ADR-038](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-038-attested-checkpoints.md)) |
| Connecting to the network | Shipped: a public WebSocket bootnode at `p2p.maya2c.dev`, and a bootstrap endpoint for snapshots and headers at `bootstrap.maya2c.dev` |
| Open registration | **Not yet.** Registrations are reviewed, because of the capture risk below |

### Why registration is reviewed for now

Today every validator seat carries one vote, whatever its stake. A
registration whose node never comes online still takes a seat, and with
enough absent seats no quorum can form and the chain stops. On a network
with one validator, one absent seat is enough
([ADR-039](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-039-quorum-n-minus-f.md)).

Until votes are weighted by stake, the testnet validator only proposes
registrations above a bond floor that the faucet cannot fund. The project
funds the bond of each approved operator. This is a stopgap for a testnet,
and it is written down as one. It is not how mainnet will work.

## Requirements

- An always-on machine. What the testnet validator itself used, measured
  on 2026-10-04 after 3 h 47 min and 12,277 blocks: 39 MB of memory, 59 s
  of CPU time (well under 1 % of one core), and 150 MB of data, which is
  about 1 GB a day at that rate with empty blocks. Busier blocks need more.
  Plan disk for growth or run with pruning. An Oracle Cloud Always Free VM
  is far above this.
- A stable connection, and an inbound TCP port you can open.
- Willingness to update within 48 hours of a release marked *required*.

## Steps

1. **Install** `maya2c-node` and `l1-wallet` from the
   [releases page](https://github.com/EricWijesinghe/Maya2C/releases)
   and check them against `SHA256SUMS`.
2. **Make a validator key.** This prints its public key:

   ```bash
   maya2c-node --generate-validator-key validator.key
   ```

   The file is the secret. Keep it on the validator machine only, readable
   by you alone, and back it up offline. Losing it costs your seat. Leaking
   it lets someone sign as you, which is slashed as double signing.
3. **Make a wallet** with `l1-wallet generate`, and note `l1-wallet address`.
4. **Apply** by opening a
   [validator application](https://github.com/EricWijesinghe/Maya2C/issues/new?template=validator-application.yml)
   with the public key and the address. Never paste a key file.
5. **Once approved and funded, register:**

   ```bash
   l1-wallet --rpc-url https://rpc.maya2c.dev/rpc register-validator \
     --validator-key validator.key --bond <the amount agreed>
   ```

   It prints your validator id. The committee is recomputed at each epoch
   boundary (every 3,600 blocks, about an hour), and you join it at the
   first boundary after the registration is final.
6. **Get the testnet genesis** from
   [maya2c.dev/testnet/genesis.json](/testnet/genesis.json) and check that
   it hashes to
   `dd9bb356c94b7699e6e8d2595681faee48be435b02dc927677cbf833e55edd5d`
   (SHA-256).
7. **Run the node.** It dials the bootnode over WebSocket, so it works
   from behind NAT. It also catches up to the network's newest attested
   checkpoint before it votes:

   ```bash
   maya2c-node --genesis genesis.json --data-dir data      --validator-key validator.key      --bootnode /dns4/p2p.maya2c.dev/tcp/443/wss/p2p/12D3KooWK2bykmqyzdK4TMSTBQ5visSjkFoK8wj88aMn3ms4ByiG      --catch-up-from https://bootstrap.maya2c.dev/rpc
   ```

## What being a validator means

- **Downtime is slashed.** A validator that commits fewer than half of the
  anchor slots it led in an epoch loses 1 % of its bond and is jailed for
  two epochs.
- **Double signing is slashed hard.** 50 % of the bond, and the key is
  retired forever. Never run the same key on two machines.
- **Your node votes on every round.** Its signatures are ML-DSA-65, the
  NIST post-quantum standard, like every other signature on the chain.
