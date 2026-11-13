# Integrating Maya2C at an exchange

Status: **mainnet has not launched.** The public network is `maya-testnet-1`.
Everything below is the node as the code stands on `master`, with the source
for each statement. **External audit pending:** no external security audit
has been completed or reported. A self-review is not an audit.

## Finality: one confirmation is final

The node is ordered by DAG-BFT (ADR-027), not by proof of work. Validators
build a certified DAG, the Bullshark commit rule picks anchors, and **each
committed anchor's sub-DAG becomes exactly one block**, derived by every node
from the same certificates (`consensus::bft::builder`). A DAG-BFT node imports
no block from gossip, `--sync-from` or `submit_block`. A node that fell behind
imports only blocks attested by a quorum of the committee it already trusts
(ADR-038, `--catch-up-from`).

So **a block your node serves as canonical is final.** There is no
confirmation-depth table and no probabilistic model:

- Safety holds while at most `f` of `n` validators are Byzantine, with
  `f = ⌊(n − 1) / 3⌋`. Liveness needs partial synchrony.
- Confirmations = finality: credit a deposit once the block carrying it is
  canonical on your own node. Waiting for more blocks adds no safety.
- Measured submit-to-finalized latency, 4 validators on **one machine over
  localhost**: p50 1,545 ms, p99 2,158 ms, 200 of 200 transfers finalized
  (`reports/bench/finality-2026-11-11-231519.md`, `cargo xtask bench-finality`).
  That is not a network figure. No WAN or multi-machine measurement exists.

### How to detect a finalized block

JSON-RPC 2.0 on the node's `--rpc-addr` (default `127.0.0.1:8545`), methods in
`crates/node/src/rpc/server.rs`:

| Method | Params | Use |
|---|---|---|
| `get_chain_info` | none | `height` is the tip, and on a DAG-BFT node the newest final height. `genesis` is the chain tag every signature commits to. `chain_id` is the network label. |
| `get_block_by_height` | `[height]` | `header.id`, `prev_hash`, and `transactions` (each with `txid`, `sender`, `nonce`, `outputs[{recipient, amount}]`). |
| `get_blocks_by_height_range` | `[from, count]` | Up to 32 blocks per call. A short answer means the node has no more, not a gap. |
| `get_balance_changes` | `[height]` | Every balance a block moved, `before` and `after`, with `block_id`. Refused for genesis and for pruned heights. |
| `get_bft_status` | none | `epoch`, `committed_round`, `committee`, `follower`. `null` on a node not running DAG-BFT. |
| `get_checkpoint` | `[]` or `[epoch]` | The newest quorum-attested block: `height`, `block` id, and ML-DSA-87 validator signatures. |

The node exposes no per-transaction finality flag. Poll `get_chain_info`, then
read each new height up to `height`. `cargo xtask bench-finality` uses exactly
this bar: a transfer is finalized when a block carrying it is canonical on a
node that never received it from a client.

For a large deposit you can also check your node against the committee: the
`block` id in `get_checkpoint` must equal `header.id` at that height on your
node. The signatures verify against the committee in state, so the serving
node is trusted for nothing.

## Deposit detection

- **One address per customer.** An address is
  `blake3("custom-l1-node.address.v3" ‖ ml_dsa_pk ‖ slh_dsa_pk)` (`spec/02-transactions.md`
  TX-2). It costs nothing to create and needs no on-chain registration.
- **No memo or tag field.** Shared-address-plus-memo schemes are not possible
  and not needed.
- **An included transaction executed.** The block builder keeps only the
  in-order transactions that execute and drops duplicates by txid (ADR-027 §2).
  A transaction that would fail is not in the block.
- **Credit from outputs, reconcile with balance changes.** Outputs to your
  addresses name the txid and the amount. Value can also arrive without a
  plain output (contract calls, vault withdrawals, HTLC claims, staking
  unbonds), so reconcile each height against `get_balance_changes`.
- **Record `(height, block id, txid)` with every credit.** Credit each txid at
  most once.

### The reference watcher is not ready for 24 decimals

`crates/deposit-watcher` credits each deposit exactly once across crashes
(append-only, fsync'd journal; tested with 100,090 deposits and 40 SIGKILLs,
`reports/17-integrations.md`). Two gaps today:

- Its `Transfer::amount` is a `u64`. One coin is 10²⁴ base units, larger than
  `u64::MAX` (about 1.8 × 10¹⁹), so it cannot hold one whole coin. It
  predates ADR-046.
- It ships no RPC `BlockSource`. You implement `finalized_height` (return
  `get_chain_info.height`) and `transfers`.

Use it as a pattern for exactly-once crediting, not as a drop-in service,
until it moves to `U256`.

## Addresses: Bech32m text, hex underneath

ADR-081, `crates/address`: `<hrp>1` + version symbol `0` + 52 payload symbols
+ 6 checksum symbols, Bech32m (BIP-350).

| Network | Chain id | HRP |
|---|---|---|
| Mainnet | `maya-mainnet` | `maya` |
| Testnet | `maya-testnet-1` | `tmaya` |
| Devnet | none registered | `dmaya` |

- The HRP is the network check: decoding for the wrong network fails with
  `WrongNetwork`. Show and accept text addresses to customers.
- Address inputs on the RPC accept hex (with or without `0x`) or text. A node
  whose chain id is not registered refuses text input.
- Outputs are still hex in most fields. `get_balance` and
  `get_account_at_tip` add `address_text`, but block `outputs`, `sender` and
  `get_balance_changes` are hex. Convert with `crates/address` before
  comparing with stored text. PR #410 (open) moves the remaining hex surfaces
  (pool login, explorer, site faucet) to Bech32m.

## Amounts: 24 decimals, 256-bit

- `maya_amount::COIN_DECIMALS = 24`: one coin is 10²⁴ base units.
- Amounts are `U256` (ADR-046) and travel as **base-10 strings** in JSON
  (`state::amount_serde`), never as JSON numbers. Parse into a 256-bit integer
  or a decimal type. A float or a 64-bit integer loses value.

## Fees

`crates/node/src/state/fees.rs`, `spec/04-fees.md`, ADR-029:

- `get_fee_info` returns `active`, `base_fee` (per serialized byte, decimal
  string) and `collector` (hex address).
- **A fee is an ordinary signed output to the collector**, not a header field.
  It must be at least `base_fee × size_bytes` of the serialized transaction
  (FEE-5). Anything above that is the validators' tip.
- The base-fee part burns. Tips pay validators at the end of the staking epoch.
- The base fee steps toward a target block size, by at most one
  `1/change_denominator` share per block (FEE-3), so leave headroom between
  quoting and broadcast. `l1-wallet` defaults to twice the base fee on the
  transaction's size.
- Where genesis configures no fees, `active` is `false` and none is paid.

## Withdrawals: signing

- Every transaction carries **two signatures over the same bytes**:
  ML-DSA-65 (FIPS 204) and SLH-DSA-SHA2-128s (FIPS 205). It authorizes nothing
  unless both verify (TX-1). The pair is 3,309 + 7,856 = 11,165 bytes
  (`crates/node/src/crypto/hybrid.rs`).
- The signed bytes include `chain_tag`, the 32-byte genesis id from
  `get_chain_info.genesis` (TX-3, TX-5). A signature is valid on one chain
  only. Pin the genesis id in your signer and refuse to sign for another.
- Suite-tagged (v7) and multisig (v8) transactions are valid from genesis
  under the suite registry's policy (TX-4).
- **Batch.** A transfer carries several outputs, so one signature pair covers
  many withdrawals and the fee's size cost is shared.
- **Nonces are strict** (STF-1): the nonce must equal the sender's current
  nonce. Run one hot account serially, or several hot accounts in parallel.
  `get_account_at_tip` returns the next nonce with the height it was read at.
- **Broadcast** with `send_raw_transaction [hex]`. It returns `txid`, and
  `accepted: false` when the transaction is already pooled. The pool checks
  signature, nonce and balance against committed state before relaying.
- **Retry safely.** Re-sending the same signed bytes cannot pay twice. A
  replacement signed with the **same nonce** (for a higher fee) also cannot:
  only one transaction per nonce executes. Never re-sign a stuck withdrawal
  with a new nonce while the old one can still land.
- Reference signer: `l1-wallet send` (`bins/l1-wallet`), which prompts for the
  keystore password and has no `--password` flag. Keys are zeroized in memory
  (Standing Order 5). Keep withdrawal keys off the node machine.

## Reorg policy

A DAG-BFT node does not reorganise committed blocks. Treat any change as a
safety incident, not as routine:

- If the block id at a height you credited ever changes, or your node's id
  disagrees with `get_checkpoint`'s attested id, **stop crediting and
  withdrawals** and escalate. It means more than `f` validators misbehaved,
  or your node is eclipsed or compromised.
- Below a pruned node's horizon a reorg is refused by design (invariant 27).
- `docs/runbooks/deep-reorg.md` still describes proof-of-work reorgs. It is
  out of date for DAG-BFT.

## Node operations

- **Run your own node** and point your watcher and signer at it, never at a
  public RPC. Keep `--rpc-addr` on loopback. The RPC rate-limits per client
  IP, and the public gateway forwards only an allow-list of methods.
- **Do not prune** the node your watcher reads (`--prune`, `--prune-depth`
  off). A pruned node refuses `get_balance_changes` for pruned heights, and
  serves pruned blocks only from a verified archive.
- **Starting from a snapshot or after an outage:** `--bootstrap-from` restores
  a pruned node from a peer's RPC. `--catch-up-from` rejoins through a peer's
  attested checkpoint (ADR-038). `--trusted-block HEIGHT:HEX` names a block the
  chain must pass through, and is required for a node stale past the
  unbonding window. Use two or more https sources.
- Node types and hardware: `docs/NODE_TYPES.md`.
- `get_node_version` returns the binary's version and supported protocol
  version, for your monitoring.

## A chain halt

If the committee stops committing, no new blocks appear and deposits stop
arriving. Nothing final is lost. A node that meets a protocol upgrade it does
not support halts with "upgrade required before height H: protocol version V
activates there and this binary supports up to version S"
(`crates/node/src/error.rs`). Upgrade the binary and restart, and it catches up.
Pause withdrawals while your node's `get_chain_info.height` is not advancing.
