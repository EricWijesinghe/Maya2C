# ADR-027: DAG-BFT in the node — derived blocks, signed votes, a safety log

**Status:** Accepted
**Date:** 2026-09-27
**Implements:** ADR-015 ("Not wired into `custom-l1-node`"), ADR-016 launch blocker 1

## Context

ADR-015 chose a certified DAG with the Bullshark commit rule and built it as a
sans-IO engine run only by the simulator. ADR-016 froze mainnet scope and
listed "wire dag-bft into the node" as the first of three launch blockers:
until it is done a `production` binary refuses to start, because the only
mode the node ran was proof of work.

ADR-015 listed what wiring needs: vote signatures, a transport, epoch changes,
a block builder, and a migration. This ADR records how each was done, and
which were deliberately not.

## Decision

### 1. A block is derived, never proposed

Each committed anchor's sub-DAG becomes exactly one block, built by every node
from the same certificates on the same parent state
(`consensus::bft::builder`). No DAG-BFT node imports a block from gossip, from
`--sync-from`, or from `submit_block` (the RPC refuses it). A block a node did
not derive is only a claim; the certificates are the proof.

This keeps `Chain`, `StateDB` and the block format unchanged. The header keeps
its proof-of-work layout and three fields change meaning:

| field | under dag-bft |
|---|---|
| `nonce` | seal: epoch (24 bits) ‖ anchor round (40 bits) |
| `timestamp` | anchor's certified `timestamp_ms / 1000`, never behind the parent |
| `difficulty_target` | the retarget rule at the unlimited floor; no work is verified |

**Why not a new header version:** every tool that parses headers (light client,
explorer, spec vectors, the TS verifier) would need a second format for no
security gain — the seal names the anchor, and the anchor's certificate is
what proves the block.

### 2. Ordered is not executable: a deterministic filter

Validators propose from their own mempools concurrently, so a sub-DAG can
carry a double spend, a stale nonce or one transaction twice. The builder
decodes payloads in commit order, drops duplicates by txid, and keeps the
longest in-order subsequence that executes (`StateDB::select_applicable`).
Every node runs the same filter on the same inputs, so the filter is part of
the ordering rule. If the survivors still fail the end-of-block passes, the
block is built empty: an anchor always yields a block, or two nodes that
disagreed about one transaction would disagree about every later height.

Cost: one overlay copy per candidate transaction (quadratic in a block's
touched state). To be measured under Master Prompt 12 before it is called
acceptable.

### 3. Signatures: ML-DSA-65, carried, not aggregated

The engine gained an `Authenticator` trait. The node implements it with
ML-DSA-65 over `"maya2c/dag-bft/vote/v1" ‖ vertex_digest`. A proposal's
signature and its author's vote are the same object. A certificate carries its
2f + 1 signatures side by side (ADR-021 measured why they are not aggregated).
The engine never trusts the transport's idea of the sender except to address a
`Fetch` reply. A second validly signed proposal for one `(round, author)` is
reported as `Equivocation` evidence (for the staking module, §6).

The simulator uses `Unauthenticated`, which signs nothing and believes
everything, and says so in its type name and docs (Standing Order 3).

### 4. Safety across restarts: fsync before send

A validator that restarts without memory would re-propose round 1 and re-vote
in slots it already voted in — an equivocation. `consensus::bft::store` keeps
two logs per epoch: `safety.log` (own proposals and votes, fsync'd **before**
the frame is returned for sending) and `certs.log` (certificates, flushed).
On restart, proposals and votes are restored first (they set the round), then
certificates are replayed through full verification. Anchors already turned
into blocks are skipped by comparing their seal with the tip's.

### 5. Transport: one gossip topic, canonical frames

`/l1/bft/1.0.0` carries hand-encoded, bounded frames
(`consensus::bft::wire`): `version epoch from to body`. Votes are addressed but
broadcast (gossipsub has no unicast); `to` saves others the work. The gossip
layer relays any frame with the right version byte and leaves signatures to
the engine.

### 6. Committee: genesis, equal weight, one epoch

The committee is `genesis.bft.validators` (hex ML-DSA-65 keys, order is
validator id), committed into the genesis id. Equal weight among members.
**Stake weighting, entry, exit and slashing are the staking module's**
(ADR-016 launch blocker 2); the safety logs are already per epoch so an engine
switch at an epoch boundary does not reuse a log.

### 7. Wall clock

The engine never reads a clock; the binary passes one in. A vertex's
`timestamp_ms` is its author's clock at proposal and is **certified**, so every
node reads the same value — the same status as a proof-of-work block
timestamp, and not a violation of Standing Order 4. The anchor timeout uses the
local clock, which shapes liveness, never safety.

## What this does not do (reported, not hidden)

- **No committee changes.** One epoch with the genesis committee.
- **No trustless historical sync.** A node more than `GC_DEPTH` (50) rounds
  behind cannot rebuild the DAG from peers' engines. Today it needs a state
  snapshot from a trusted peer. Serving stored sub-DAGs as block
  justifications, verified by replaying the commit rule, is the planned fix.
- **Gossip admission is by decode only.** Garbage that decodes is relayed
  before the engine rejects it; peer scoring bounds the cost.
- **Inline payload.** Vertices carry transactions (bounded by
  `max_batch_bytes`), not Narwhal worker-batch digests. Simpler; caps
  throughput at what one proposer's uplink carries.

## Consequences

- `maya2c-node` runs DAG-BFT when genesis has a `bft` section, as a validator
  (`--validator-key`) or observer. `--generate-validator-key` writes a key.
- A `production` binary now starts on a DAG-BFT genesis and still refuses a
  proof-of-work one.
- Tests: `crates/dag-bft/tests/auth_and_restart.rs` (forged votes and
  certificates, equivocation evidence, restart without re-signing, observer);
  `crates/node/tests/bft_node_tests.rs` (4 ML-DSA validators + observer on
  real RocksDB chains: identical blocks, a transfer executes, a double spend
  lands once, restart from the safety log without equivocation);
  `scripts/bft_devnet.py` (the real binary, five processes, localhost).

## Revisit when

- Staking lands (committee from state at epoch boundaries), or
- a measured throughput ceiling shows inline payload is the bottleneck, or
- trustless historical sync is built.
