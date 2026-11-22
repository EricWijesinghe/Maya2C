# ADR-123: Blob transactions with expiry, and the availability record a light client can prove

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-35, [prior-art/blobs-and-da.md](../prior-art/blobs-and-da.md), [ADR-072](ADR-072-max-block-bytes.md), [ADR-063](ADR-063-block-header-version-and-time-bound.md), [ADR-029](ADR-029-fee-market-live.md), [ADR-119](ADR-119-data-carrier.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 17, 24, 25, 27, 28, 31, 33

## Context

`crates/da` lays a block's data out as a `k × k` square, extends it with
Reed–Solomon to `2k × 2k`, commits to row and column roots with BLAKE3 (no
KZG: "needs a trusted setup and falls to a quantum computer", `lib.rs:13-23`),
and derives one `data_root` (`square.rs:19-27`). It samples, repairs and
detects a bad encoding (`square.rs:137`), but "nothing in consensus produces
or checks these squares yet" (`lib.rs:25`; `features.toml:3120-3128`
RESEARCH), and the bad-encoding fraud proof is not a gossip message.

Blocks are capped at 5 MiB of wire bytes (invariant 33), every byte pays the
burning base fee (ADR-029), and a pruned node deletes bodies below its
horizon only after an archive holds a verified copy (invariant 27,
`state_pruner/mod.rs:5-31`). The header is fixed-width with a version byte
(`block.rs:181`, `:207-230`; ADR-063). A payload rides a DAG-BFT vertex whose
batch is bounded by `Params::max_batch_bytes`, one quantum (ADR-072).

Ethereum's EIP-4844 carries blobs in a sidecar, keeps them about 18 days,
prices them in a second EIP-1559 market, and leaves a KZG commitment on
chain forever; Celestia commits row/column roots in the header and attests
them to other chains (prior-art file).

**Findings on the brief.**
1. "DA attestations exportable to L2s": Maya2C has no L2 and no rollup
   framework in this tree; the attestation defined below has no consumer
   here. It is a proof format, and the doc says so.
2. "Needs its own fee lane": agreed, and it is not ADR-121's lane. A blob
   market is a second base fee over blob bytes, not reserved block space.
3. Light-client sampling needs the bad-encoding fraud proof the crate does
   not package. Consensus does not: validators rebuild the square. Sampling
   is a later step, named in "What this is not".

## Decision

**A blob transaction carries a 32-byte `data_root`, `k`, and the byte
length; the bytes travel in a sidecar next to the vertex and never enter the
block. A validator votes on a vertex carrying a blob only after it holds the
sidecar, re-encodes the square and reproduces `data_root`. The commitment,
its expiry height and the block that carried it are a committed record under
the state root for `BLOB_RETENTION_BLOCKS`; the bytes are local-only and
deleted at expiry without archive. Blob bytes pay a separate base fee that
steps on blob bytes per block.**

### Wire and consensus

- `TxKind::Blob { data_root: [u8; 32], k: u16, len: u32 }`, v6 payload
  frame (no wire version). `k <= MAX_BLOB_K = 64`, `len <= MAX_BLOB_BYTES =
  512 KiB`, and `len` must equal what a `k × k` square of
  `BLOB_CHUNK = 256`-byte chunks holds after padding (`k² × 256`), so the
  square is determined by the transaction.
- **Sidecar.** A vertex's wire form (`consensus/bft/wire.rs:176`) gains an
  optional `sidecars: Vec<(txid, bytes)>`, one per blob payload in the
  batch, bounded by `MAX_BLOB_BYTES_PER_VERTEX = 1 MiB` outside the
  `max_batch_bytes` meter (the meter bounds decode work on transactions; a
  blob is not decoded, it is re-encoded, and its own bound prices that).
  A validator **refuses to vote** on a vertex whose sidecar is missing,
  over the bound, or whose re-encoding (`Extended::encode`, `square.rs:50`)
  does not reproduce each `data_root`. That is the availability guarantee:
  a certified vertex was held and checked by `n − f` validators.
- **Per-block blob bound.** The decode walk admits at most
  `MAX_BLOB_BYTES_PER_BLOCK = 2 MiB` of blob bytes (governed under a compiled
  ceiling; invariant 17); blobs past it stay with their author for the next
  walk, as ADR-072's misfit rule does.

### State (invariant 25)

Prefix `bl:` in a new layer `StateLayer::Blobs`:

- `bl:c:` ‖ `expiry(8 BE)` ‖ `data_root` → `BlobRecord { height, k, len,
  sender }`. The attestation. Deleted by the end-of-block pass in key order
  once `expiry <= height`; `expiry = height + BLOB_RETENTION_BLOCKS`
  (`1_036_800`, 12 days at 1 s blocks; question 2).
- `bl:fee` → the blob base fee, stepped per block by
  `maya_fee_market::next_base_fee` over `(blob bytes in block,
  target_blob_bytes)` with the ADR-076 clamp and ceiling, `target_blob_bytes`
  governed (`MAX_TARGET_BLOB_BYTES = MAX_BLOB_BYTES_PER_BLOCK / 2`).

Local-only prefix `blob:` ‖ `data_root` → the bytes, in `LOCAL_ONLY_PREFIXES`
(`commitments.rs:100`), written when the vertex is certified, deleted by the
same expiry pass. **Invariant 27 is not touched**: blob bytes are not a block
body; the expiry is the promise, written in the record, and an archive node
*may* keep them (`ArchivePolicy` gains `blobs: bool`) but no node must.

### The attestation

A `StateLayer::Blobs` proof (`state/proof.rs:311`) of `bl:c:` ‖ expiry ‖
root under a finalized header's `state_root` says: "at height `h`, `n − f`
validators held bytes whose square commits to `data_root`, and they are
served until `expiry`". That is what Celestia's Blobstream exports, in
Maya2C's own Merkle form and without a destination-side verifier (the same
gap ADR-122 names). RPC: `blob(data_root)` returns the bytes or `Expired`,
and `blob_proof(data_root)` the layer proof. A sampler asking for one cell
(`square.rs:112`) is served from the stored square by `da_cell(data_root,
row, col)`.

### Fees

A blob transaction pays the ordinary byte fee on its ~50-byte wire form and
`bl:fee × len` on its blob bytes as an output to `FEE_COLLECTOR`, the base
part burned like ADR-029's. Two markets, two controllers, one collector.

## Alternatives considered

- **Blobs inside the block (ADR-119 with a bigger cap).** Permanent bytes
  at the permanent price, and every archive forever. The point of a blob is
  that it expires.
- **KZG commitments (EIP-4844).** Trusted setup, not post-quantum; the
  crate already refused it.
- **A `data_root` field in the header (Celestia).** Needs `HEADER_VERSION
  = 1` and a 145-byte header everywhere (ADR-063; `state_pruner/mod.rs:20`).
  The state root already commits to `bl:c:`; the header is unchanged.
- **Keep blobs under invariant 27 (archive before delete).** Then a blob is
  not expiring data but a second block body. The record says the expiry;
  archiving is a policy.
- **Light-client sampling now.** Needs the fraud-proof gossip; a sampler
  without it can be fooled by a bad encoding that full validators would
  have caught. Later, with its own ADR.

## Consequences

- Rollup-style and app data has a priced, expiring, provably-held home;
  2 MiB per block of it, outside the 5 MiB block.
- A vertex wire change (sidecars), a vote precondition in
  `consensus/bft/auth.rs` and `driver.rs`, one kind, one layer, one local
  prefix, one fee record and controller, one expiry pass, three RPCs.
- Validators hold up to `2 MiB × BLOB_RETENTION_BLOCKS` ≈ 2 TiB at full
  load; question 2 is about that number. At today's measured demand the
  figure is MISSING, and the retention is the lever.
- `crates/da` leaves RESEARCH only when the vote precondition test below
  passes on the node; its `features.toml` row changes then.

## Invariants touched

- **24**: `bl:c:` writes and the fee step are in the only apply path.
- **25**: `bl:` layer; `blob:` on the local-only list; undo covers `bl:`.
- **27**: explicitly not extended to blob bytes; recorded here and in the
  invariant's text at implementation.
- **17**: `target_blob_bytes` and `MAX_BLOB_BYTES_PER_BLOCK` governed under
  compiled ceilings.
- **7**: a blob whose sidecar fails is a vertex not voted for, never a
  block refused after certification (the check is before the vote).
- **28**: `Module::Blobs`; a breaker admits no blob and runs no expiry.
- **31**: v6 payload kind; no frame change.
- **33**: the block carries ~50 bytes per blob; the sidecar is outside it.

## Activation

`FeatureSet::BLOBS = 1 << 30` (claimed in `adr-claims.txt`), compiled
`BLOBS_ACTIVATION_HEIGHT = u64::MAX`, scheduled under ADR-082 with the v4
bundle at **genesis + 15,552,000**. Before activation a `Blob` kind is
refused and a vertex with sidecars is refused at the wire.

## Test plan

Red tests, `crates/node/tests/blob_tests.rs` and `consensus/bft`:

- `a_vertex_with_a_blob_and_no_sidecar_gets_no_vote`
- `a_sidecar_that_does_not_reproduce_the_root_gets_no_vote`
- `a_certified_blob_is_committed_with_its_expiry_and_served_by_rpc`
- `a_blob_is_deleted_at_expiry_and_rpc_says_expired`
- `the_record_outlives_a_prune_below_the_horizon_until_expiry` (27 untouched)
- `the_blob_fee_steps_on_blob_bytes_and_not_on_transaction_bytes`
- `blobs_past_the_per_block_bound_wait_with_their_author`
- `a_layer_proof_of_the_record_verifies_against_the_header`
- `a_reorg_restores_records_and_the_fee` (bytes are local; not restored)

`crates/da/tests/da_tests.rs` gains an `Extended::encode` vector at `k =
64`. Vectors (`spec/03-state.md` BL-1..BL-2): a record; a fee step.

## Open questions for Eric (economics only)

1. `target_blob_bytes` and the blob fee floor at activation.
2. `BLOB_RETENTION_BLOCKS`: 12 days as written (the storage figure above),
   or Ethereum's ~18 days?
3. Does the blob base fee burn (as written) or fund the treasury (ADR-111),
   given validators bear the storage?
