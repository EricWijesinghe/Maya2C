# ADR-038: Attested checkpoints, so a validator can rejoin

**Status:** Proposed (2026-10-04). Mainnet launch gate 8
(`docs/mainnet-v1-plan.md`).
**Date:** 2026-10-04

## Context

The 2026-10-04 four-validator dry run (`maya-mainnet` and `maya-rehearsal-1`
geneses minted by `bins/genesis-ceremony`, four `maya2c-node` processes)
showed that a DAG-BFT validator offline for longer than about half a minute
never rejoins:

- A validator 5 s down rejoined. One 90 s down stayed at its last height for
  good while the other three went on (300+ blocks later, still stranded).
- The engine keeps `GC_DEPTH` = 50 rounds of certificates in memory and
  answers `Fetch` only from there (ADR-027, "no trustless historical sync").
- Serving older certificates from `certs.log` over gossip was tried. Peers
  found and sent them (every one verified as the certificate asked for), but
  the replies did not arrive: gossip floods every reply to every node,
  de-duplicates identical retries, and a healthy validator serving the
  lagging one dropped 442 frames to its own event channel in two minutes
  (#48 makes such losses visible).
- Bootstrapping the validator from a peer's state snapshot does not help: a
  snapshot is served at least `--prune-depth` blocks deep (100 in the test),
  about 250 rounds, which is already outside every peer's 50-round window.

On this chain a block is a function of certificates, and a node never imports
a block it did not derive — so a node that missed the certificates can never
derive the blocks. With four validators, one unplanned outage costs the
network its fault tolerance and a second halts it. That cannot carry three
months of uptime.

## Decision

**Validators attest to blocks; a quorum-attested block is a checkpoint any
node may import up to.**

- After a validator builds and inserts block `B` at height `h`, it signs
  `"maya2c block attestation v1" ‖ chain_tag ‖ epoch ‖ h ‖ id(B)` with its
  DAG-BFT validator key (ML-DSA-65; the remote signer too) and gossips the
  attestation. Every validator builds the same `B`, so the signatures agree.
- `2f + 1` attestations from the epoch's committee make `B` a **checkpoint**.
  Nodes keep the newest few checkpoints with their signatures, not one set
  per block: blocks are hash-chained, so a checkpoint vouches for every
  ancestor.
- A node behind the engine's window asks a peer for the newest checkpoint,
  verifies its quorum against the committee in its own state, then fetches
  the missing blocks by height (the existing `get_block_by_height`), checks
  they hash-chain to the checkpoint, and **re-executes** each one through
  `Chain::insert_block`, which refuses a block whose declared state root its
  own execution does not reproduce (invariant 24). The peer is trusted for
  nothing: the quorum says which chain, re-execution says the state.
- Once within the engine's window of the tip, the validator resumes the
  engine at the checkpoint's seal and fetches the last rounds from peers'
  memory, which already works (the 5 s case).

## Alternatives rejected

- **A much larger `GC_DEPTH`.** Certificates carry their payloads; hours of
  rounds in memory is gigabytes per node, and the gossip transport for the
  fetches still fails as measured.
- **Certificate fetch over a request-response protocol.** Sound, and still
  worth having, but replaying hundreds of rounds of the DAG is slower and
  more code than importing blocks under a quorum.
- **Trusted snapshot only.** Measured not to work (above), and it makes one
  peer's word the state.

## Consequences

- New wire message, a small store for recent checkpoints, an RPC to fetch the
  newest, and a catch-up path in the node. Consensus-adjacent: spec text,
  conformance vectors for the attestation bytes, and the rust-reviewer,
  security-reviewer and blockchain-security-auditor passes.
- Light clients and the explorer gain a finality proof for the tip.
- An attestation is not a vote in the protocol: equivocating on one (two
  different block ids at one height) is evidence of a broken node, kept for
  the staking module like vertex equivocations.

## Test plan

- Engine-free unit tests: attestation bytes (vectors), quorum counting,
  duplicate and foreign-signer refusal, hash-chain check.
- `bft_node_tests`: four validators, hold one back 300 rounds, release it;
  it reaches the others' tip and builds the same next block.
- The dry-run rehearsal with real binaries, repeated: 90 s outage rejoins.
