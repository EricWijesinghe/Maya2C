# Pruning, archives, and pruned bootstrap

A node can drop old block bodies and undo journals once a verified copy of them
exists elsewhere, fetch any of them back on demand, and a new node can join
from a state snapshot without downloading history. This is off by default; an
archive node, the default, prunes nothing and behaves as before.

## What had to be true first

Three things were missing, and each is a separate commit:

| Gap | Fix |
|---|---|
| **No block was persisted anywhere.** `Chain` held every block in memory. A node that had applied one block could not restart: `seed_state` rewrote the genesis allocations over the evolved state, then refused to start. | The block store (`src/state/blocks.rs`): headers, bodies, a canonical index and the tip, in the same RocksDB and the same `WriteBatch` as the state they describe. `Chain::open` rebuilds from it. |
| **The state root did not cover contract code, contract storage, the nullifier set, or the shielded pool's anchors and balance.** A snapshot could have forged any of them. | Invariant 25: two new root layers and a whole-pool commitment (`src/state/commitments.rs`). |
| **A block's header did not commit to its transactions.** An archived body could not have been tied to anything. | Invariant 24: `tx_root`. |

## The horizon

This chain is proof of work, and nothing is final. A pruned node adopts a
local policy instead. Blocks at least K below the tip (`PRUNE_DEPTH`, one DAG
epoch, 30,000 blocks, about 5.2 days) lose their bodies and undo journals. The
node then refuses any block or reorg that reaches at or below its horizon
(`NodeError::BelowPruneHorizon`), because the undo journals it would need are
gone.

The check sits in `Chain::reorganize`, before anything is reverted, so a reorg
that cannot finish never starts. Headers stay forever, at 144 bytes each,
about 4.3 MB per 30,000 blocks. They carry the `state_root` and `tx_root` that
a pruned body is later verified against.

## Archive, verify, then delete

`state_pruner::archive::prune_round` runs one round:

1. Picks the next batch deep enough (default 1,000 blocks) and reads the bodies.
2. Builds a CAR v1 archive (`maya-archive`). The root is a DAG-CBOR manifest
   linking every block by CID. CIDs are CIDv1 over BLAKE3-256: `raw` for
   blocks, `dag-cbor` for the manifest.
3. Writes the archive to **every** configured store:
   - the local directory, as `.car.zst`;
   - optionally a kubo IPFS node, as plain CAR through
     `/api/v0/dag/import?pin-roots=true`.

   Then it **reads every copy back and verifies it** against the root.
4. Only then deletes the bodies and undo journals, records the receipt, and
   raises the horizon, all in one batch. `Chain::prune` first checks the active
   chain still holds exactly the archived blocks. A reorg during the upload
   turns the round into a no-op.

A store that fails fails the round, and nothing is deleted. Pruning with no
archive at all (`--prune-without-archive`) is possible, as it is for a Bitcoin
pruned node, but it has to be asked for.

## Fetching a pruned block back

`state_pruner::cold::ColdBlocks::fetch` serves `get_block_by_height` for a
pruned height. Archives come from stores nobody has to trust, so a block is
returned only if:

- every CAR section hashes to its CID;
- the archive's root is the one in the receipt this node wrote;
- the decoded block's id is the id of **the header this node kept**, and its
  transactions match that header's `tx_root`.

Arweave is supported for reading, through a gateway (`--arweave-gateway`).
Uploading to Arweave signs with a wallet and pays AR per byte, so it is never
done automatically. Upload the `.car.zst` with your own tooling if you want
that copy.

## Bootstrapping a pruned node

`--bootstrap-from <url>` on an empty data directory runs
`state_pruner::snapshot::bootstrap_pruned`:

1. Every header from genesis to the source's tip, fully validated: the exact
   retarget rule, proof of work, cumulative work.
2. A snapshot at a height H at least K below the tip. Each chunk is checked
   against the manifest, only keys under committed prefixes are accepted, keys
   must strictly increase, and the recomputed state root must equal
   `header(H).state_root`. Otherwise everything imported is deleted and the
   bootstrap fails.
3. The bodies from H + 1 to the tip, applied with full validation.

No body at or below H is ever requested; `tests/pruned_node_tests.rs` records
every request to prove it. What remains is the SPV limitation: a source showing
only a lower-work fork of headers. The answer is the one a light client has:
ask more than one peer.

A node serves snapshots with `--snapshot-interval <N>`. That takes a RocksDB
checkpoint every N blocks and serves the newest one that is at least K deep.

## Flags

| Flag | Effect |
|---|---|
| `--prune` / `--prune-depth <N>` | Prune bodies older than 30,000 / N blocks |
| `--archive-dir <PATH>` | Local archive directory (default `<data-dir>/archive`) |
| `--ipfs-api <URL>` | Also archive to kubo, e.g. `http://127.0.0.1:5001`. The RPC API is administrative: loopback only |
| `--arweave-gateway <URL>` | Also fetch archives from an Arweave gateway |
| `--prune-without-archive` | Prune without writing any archive |
| `--snapshot-interval <N>` | Take and serve a snapshot every N blocks |
| `--bootstrap-from <URL>` | Bootstrap a pruned node from a peer's JSON-RPC endpoint |

## Not done

- **Arweave upload.** It needs a wallet and spends AR.
- **A libp2p snapshot protocol.** JSON-RPC carries snapshots for now, as it
  already carries backfill.
- **Per-chunk range proofs.** A snapshot is verified whole. A lying server
  wastes bandwidth but cannot inject state.
- **A live IPFS round trip.** `KuboStore` is tested against a mock of kubo's
  two endpoints. No kubo daemon has been run here.
