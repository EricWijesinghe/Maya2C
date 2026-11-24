# ADR-118: Compressed collections — one root on chain, items proven by witness

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-21, [prior-art/state-compression.md](../prior-art/state-compression.md), [ADR-062](ADR-062-state-layout-before-genesis.md), [ADR-103](ADR-103-native-fungible-tokens.md), [ADR-104](ADR-104-unique-assets.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 24, 25, 28, 33; Standing Order 4

## Context

Every state record is a RocksDB key under a committed prefix, folded into the
state root per layer (`crates/node/src/state/commitments.rs:44-66`,
invariant 25). ADR-104 stores one `n:i:` record (32-byte owner, 32-byte
metadata hash) per item and one `n:o:` owner-index record per holding; ADR-103
stores one `d:bal:` ‖ asset ‖ address record per holder
(`crates/node/src/state/dex.rs:236-238`). A collection of ten million items is
ten million committed records that every full node stores and every snapshot
carries (`state_pruner/snapshot.rs:51`, 4,096 entries per chunk). The accounts
tree is rebuilt dense each block today (ADR-062, `state/merkle.rs:1-14`); the
sparse tree with witness-based updates exists in `crates/stateless-core`
(`sparse.rs:39` `root`, `:53` `open`) and switches on by
`STATELESS_ACTIVATION_HEIGHT` (`state/stateless.rs:5-12`).

Solana's compressed NFTs keep one Merkle root in an account and the leaves in
the transaction log, with indexers rebuilding the tree; Light Protocol's ZK
compression generalises that to any account with a Groth16 proof over a
Poseidon tree (prior-art file). Neither fits here as-is: Groth16 needs a
trusted setup and is not post-quantum, and Poseidon is not a hash this tree
has vectors for.

**Finding on the brief.** The row says "state compression for mass NFT/account
issuance". *Account* compression is not proposed: an account's balance is
read by every transfer and every end-of-block pass, and a witness-carrying
transfer would change the plain-transfer wire form (invariant 31) and the
stateless-verifiable set (`stateless.rs:14-31`). Compression here is for
collections — items and per-asset holdings — whose records are touched only by
their own operations.

## Decision

**A collection may be created compressed: the chain stores one 32-byte root
and a leaf count; items are leaves of a BLAKE3 sparse Merkle tree whose
contents live in the transaction log. An operation on a compressed item
carries the leaf and its witness; the node checks the witness against the
stored root, applies the change, and writes the new root. Indexers rebuild
the tree from the log.** Hash-based only, no proving system, no setup.

### State (invariant 25)

New prefix `cs:` in a new committed layer `StateLayer::Compressed` (tag
assigned at implementation), in `RECORD_LAYERS` and the undo journal:

- `cs:r:` ‖ `collection_id(32)` → `CompressedRoot { root: [u8; 32], leaves:
  u64, depth: u8, kind: u8 }`; `kind` is `0` items (ADR-104 shape) or `1`
  holdings (ADR-103 shape). `depth <= 32` (at most 2^32 leaves).
- `cs:h:` ‖ `collection_id` ‖ `height(8 BE)` → the root after the last change
  in that block, kept for `ROOT_HISTORY_BLOCKS = 64` and deleted by the
  end-of-block pass in key order. This is the concurrency buffer: a witness
  built against a root up to 64 blocks old is still accepted if its path does
  not intersect the changes since (the same idea as Solana's changelog,
  prior-art file). Without it two users of one collection in one block
  would race.

The leaf is `blake3::derive_key("maya2c compressed leaf v1", index ‖ payload)`;
an empty leaf is the all-zero hash; internal nodes are tagged as in
`state/merkle.rs:20-22` (second-preimage rule kept). `payload` is the
ADR-104 `Item` or the ADR-103 balance record, byte for byte, so an item is
"the same record, elsewhere".

### Operations

| TxKind | Rule |
|---|---|
| `CreateCompressed { kind, depth, metadata_hash }` | Writes `cs:r:` with the empty root for `depth`; fee is bytes only. |
| `AppendLeaf { collection, payload, witness }` | `leaves < 2^depth`; witness proves index `leaves` is empty under the root (or a `cs:h:` root with no intersecting change); writes the new root, `leaves += 1`. Only the collection creator (ADR-104 `Collection.creator`), as minting is today. |
| `UpdateLeaf { collection, index, old, new, witness }` | Witness proves `old` at `index`; the signer must be `old.owner` (kind 0) or the holder (kind 1); the change rule is ADR-104's `TransferItem` or ADR-103's transfer for that `kind`, applied to `old` to produce `new`, which the node recomputes and compares, never trusts. |
| `Decompress { collection, index, leaf, witness }` | Proves the leaf, writes it as an ordinary `n:i:` / `d:bal:` record, and sets the leaf to a tombstone hash. One-way per leaf. |

Witness length is `depth` hashes: at most 1,024 bytes. A payload is at most
`MAX_COMPRESSED_PAYLOAD = 256` bytes, so a transaction grows by at most ~1.3
KiB (invariant 33 is unchanged; the byte fee pays for it).

A failed witness is a refused transaction (invariant 7: an error, not a
no-op). Every operation is `Module::Compressed` for the breaker (invariant
28), a new arm with no wildcard.

### What indexers do

The leaf payload is in the transaction, which is in the block, which every
archive node keeps (invariant 27). An indexer replays `cs:` transactions and
holds the full tree; a wallet asks it for a witness. The chain never needs the
indexer: a user with the block history builds the witness alone. The RPC
gains `compressed_root(collection)` and nothing else; witness serving is an
indexer API (`crates/indexer`, ADR-092, PR pending).

## Alternatives considered

- **ZK compression (Light Protocol).** Succinct, but Groth16 + Poseidon are
  neither post-quantum nor in this tree's vector set. A STARK over BLAKE3
  would be post-quantum and is a research line (ADR-079), not a v4 item.
- **Store leaves under `cs:l:` too.** Then nothing is compressed; it is
  ADR-104 with a root. Rejected.
- **Compress accounts.** Changes the transfer path and the stateless set
  (Context). Rejected for v4.
- **No root history.** Two writers in one block need serialised witnesses;
  with 64 blocks of history a wallet's witness survives ordinary contention.
  The cost is 40 bytes per changed collection per block, bounded by the
  pass that deletes them.
- **Dense tree per collection.** O(leaves) per update; the sparse tree is
  O(depth). `stateless-core::sparse` is what ADR-062 chose for accounts.

## Consequences

- A ten-million-item collection costs the chain 49 bytes of committed
  state plus the history records; the bytes move to the block log, which
  archive nodes keep anyway.
- Four new `TxKind` arms, one layer, one prefix family, one end-of-block
  deletion pass (key order, bounded by the number of collections changed in
  the window).
- Users depend on an indexer for *convenience*, never for *validity*; the
  docs say so plainly.
- A snapshot carries roots only, so a pruned node syncs a huge collection in
  one chunk.

## Invariants touched

- **24**: root updates are in the only apply path; a declared `state_root`
  must reproduce them.
- **25**: `cs:` prefix and layer; undo covers `cs:r:` and `cs:h:`.
- **7**: a bad witness refuses the transaction.
- **28**: `Module::Compressed`.
- **33**: witness bounded by `depth <= 32`; payload by 256 bytes.
- Standing Order 4: key-order history deletion, no map iteration.

## Activation

`FeatureSet::COMPRESSED_STATE = 1 << 25` (claimed in `adr-claims.txt`),
compiled `COMPRESSED_STATE_ACTIVATION_HEIGHT = u64::MAX`, scheduled under
ADR-082 with the v4 bundle at **genesis + 15,552,000**, and never before bits
11 (`ASSET_MINT`) and 12 (`NFT`), whose record shapes it reuses. Before
activation all four kinds are refused.

## Test plan

Red tests, `crates/node/tests/compressed_state_tests.rs`:

- `an_append_with_a_stale_but_disjoint_witness_is_accepted_within_64_blocks`
- `an_append_with_a_stale_intersecting_witness_is_refused`
- `a_forged_leaf_under_a_valid_path_is_refused`
- `update_recomputes_the_new_leaf_and_refuses_a_claimed_one`
- `decompress_writes_the_ordinary_record_and_tombstones_the_leaf`
- `a_second_decompress_of_the_same_leaf_is_refused`
- `root_history_is_deleted_in_key_order_after_the_window`
- `a_reorg_restores_root_count_and_history`
- `a_block_whose_root_omits_a_root_update_is_refused` (invariant 24)

`crates/stateless-core/tests` gains an empty-leaf and tombstone vector.
Vectors (`spec/03-state.md` CS-1..CS-3): an empty root per depth; one append;
one update. `cargo xtask spec-coverage` lists them.

## Open questions for Eric (economics only)

1. Should `CreateCompressed` carry a deposit (refunded on the first
   `Decompress`?) to price the per-collection root history, or is the byte
   fee enough? The history is bounded, so the default is byte fee only.
2. `ROOT_HISTORY_BLOCKS` (64) compiled or governed with a compiled ceiling?
