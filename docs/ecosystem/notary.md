# Notary: prove a file existed, without showing the file

`maya2c notary` timestamps a file on the Maya2C chain. It proves that this
exact file existed no later than a certain time; a matching hash shows the
file you hold is the one stamped.
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

`setup` and `stamp` each wait until their transaction is in a block (up to
two minutes) before they report success, so you can run `stamp` straight
after `setup`, or two stamps in a row. If the wait runs out, the command
fails and says the transaction was not included; check before you stamp
again. Do not run two stamps from one wallet at the same moment in two
terminals: they would share a nonce, and only one would land.

The command prints a transaction id (`txid`). Keep it. The file is hashed on
your machine, and only the hash goes into the transaction. `setup` and `stamp`
sign and pay a fee, so both ask you to type `yes` first; pass `--yes` in
scripts, which have no terminal. Set `L1_WALLET_PASSWORD` for scripts;
otherwise you are prompted for the password.

## Verify a stamp

```
maya2c notary verify contract.pdf --tx <txid> --rpc http://NODE:8545 --genesis <genesis-hex>
```

This checks the node's genesis as `stamp` does, recomputes the hash of the
file, finds the transaction in the chain, checks its signature, and prints a
receipt as JSON:

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

- `timestamp` is the block time. Under DAG-BFT it is the median of the
  validators' vertex times for that block, and never earlier than the parent
  block's time. It is honest only while the BFT assumption holds (more than
  two thirds of the committee's weight honest).
- `finality` is `final` when the node's newest committee checkpoint is at or
  above the block, and the block the node serves at the checkpoint's height
  is the one the checkpoint names. Otherwise it is `pending`.

If the file has changed by even one byte, verify fails with
"the file does not match the stamped hash", and prints no receipt.

## Check the hash yourself

You do not need this tool to check a receipt. Any BLAKE3 tool gives the same
hash, for example `b3sum contract.pdf`. Compare it with `hash` in the receipt.

## Limits to know

- **Verify is slow on long chains.** The node has no lookup by transaction
  id, so verify scans blocks from `--from` (default block 1) in batches of
  32. Run verify once after the stamp lands and keep the receipt's `height`;
  pass it as `--from` next time. On a public node, a long scan can hit the
  per-IP rate limit (HTTP 429) and stop; start nearer the stamp, or use your
  own node.
- **The chain is the node's word too.** Verify pins the genesis, checks the
  stamp's signature, and checks each block's transactions against the header
  its id hashes. It does not yet check that the headers link to each other
  or that the height is real, so a hostile node could serve a made-up chain
  under the right genesis. The block time and height are the node's word:
  a dishonest node can place a real stamp in an invented block with an
  earlier time (backdating). Until checkpoint signatures and the header chain
  are verified, use a node you run.
- **Finality is the node's word.** The receipt does not yet check the
  committee signatures on the checkpoint. Treat `final` as "the node says
  so" until that check lands. Verify does compare the checkpoint's block id
  with the block it is served at that height, so a node on another branch
  does not report `final`; it does not yet check that the stamp's block is
  an ancestor of that block (the header links). The rule is unit-tested
  (`finality_needs_a_checkpoint_at_or_above_the_block`,
  `a_checkpoint_counts_only_for_the_block_this_verifier_was_served`), and the
  devnet test waits for `final` end to end.
- **One DID signs all stamps.** Stamps from one wallet share a schema record
  on the node, and each new stamp replaces that record. The stamp
  transaction in its block is the proof, not the record. Do not rely on the
  node's state for old stamps; keep the txid and receipt.
- **Mainnet genesis is not pinned yet.** Until it is, pass `--genesis`.
- **An early miss, likely explained.** An early manual run never saw a stamp
  land in a block. The devnet test did not reproduce it (10 of 10 runs passed
  on 2026-11-21). A likely cause, found by reading the code and not
  reproduced: `stamp` run straight after `setup` reused setup's nonce, so one
  of the two was dropped. Both commands now wait for inclusion, and the
  devnet test runs setup and two stamps back to back.

## Why there is no node change

The chain already has a transaction type that carries a 32-byte root under a
label (`AnchorAttestation`, tag 34). The notary uses it with the label
`notary-v1`. A later node change could add a lookup-by-txid RPC and a
dedicated notary transaction, so verify becomes one call. That is a
post-genesis change and is not part of this work.
