# Notary: prove a file existed, without showing the file

`maya2c notary` timestamps a file on the Maya2C chain. It proves that the
file existed at a certain time, and that nobody has changed it since.
The file itself never goes to the chain or to the node. Only its 32-byte
fingerprint (its BLAKE3 hash) does.

## What it proves, and what it does not

- **Proves:** this exact file existed no later than the block time, and the
  wallet's DID signed that claim.
- **Does not prove:** who wrote the file, that its contents are true, or that
  it was not created earlier and stamped later. A stamp shows "no later than".
- **Keeps private:** the file contents. Anyone can recompute the hash of a
  copy they hold, so the copy you show someone is checkable without a server.

## Before you start

You need a wallet keystore with some funds (if the chain charges fees), and
the chain's genesis id. The node must be reachable over JSON-RPC.

Each wallet needs a DID once. The DID is what the stamp is signed under:

```
maya2c notary setup --keystore wallet.key --rpc http://NODE:8545 --genesis <genesis-hex>
```

## Stamp a file

```
maya2c notary stamp contract.pdf --keystore wallet.key --rpc http://NODE:8545 --genesis <genesis-hex>
```

The command prints a transaction id (`txid`). Keep it. The file is hashed on
your machine, and only the hash goes into the transaction. Set
`L1_WALLET_PASSWORD` for scripts; otherwise you are prompted.

## Verify a stamp

```
maya2c notary verify contract.pdf --tx <txid> --rpc http://NODE:8545
```

This recomputes the hash of the file, finds the transaction in the chain,
and prints a receipt as JSON:

```json
{
  "hash": "...64 hex...",
  "txid": "...64 hex...",
  "sender": "...64 hex...",
  "schema": "notary-v1",
  "height": 1234,
  "block_hash": "...64 hex...",
  "timestamp": 1700000000,
  "finality": "final"
}
```

- `timestamp` is the block time. It is the node's clock at block production.
- `finality` is `final` when a committee checkpoint covers the block, and
  `pending` until then.

If the file has changed by even one byte, verify fails with
"the file does not match the stamped hash", and prints no receipt.

## Check the hash yourself

You do not need this tool to check a receipt. Any BLAKE3 tool gives the same
hash, for example `b3sum contract.pdf`. Compare it with `hash` in the receipt.

## Limits to know

- **Verify is slow on long chains.** The node has no lookup by transaction
  id, so verify scans blocks from `--from` (default block 1) in batches of
  32. Pass `--from` near the stamp height if you know it.
- **Finality is the node's word.** The receipt does not yet check the
  committee signatures on the checkpoint. Treat `final` as "the node says
  so" until that check lands.
- **One DID signs all stamps.** Stamps from one wallet share a schema record
  on the node, and each new stamp replaces that record. The stamp
  transaction in its block is the proof, not the record. Do not rely on the
  node's state for old stamps; keep the txid and receipt.
- **Mainnet genesis is not pinned yet.** Until it is, pass `--genesis`.

## Why there is no node change

The chain already has a transaction type that carries a 32-byte root under a
label (`AnchorAttestation`, tag 34). The notary uses it with the label
`notary-v1`. A later node change could add a lookup-by-txid RPC and a
dedicated notary transaction, so verify becomes one call. That is a
post-genesis change and is not part of this work.
