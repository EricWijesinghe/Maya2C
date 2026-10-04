# ADR-038: Attested checkpoints, so a validator can rejoin

**Status:** Accepted (2026-10-04), implemented; evidence below. Mainnet launch gate 8
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

## Evidence (2026-10-04)

Implemented as: `consensus::bft::attest` (attestations, checkpoints,
collector), the remote signer's `Attestation` kind with its own protection
database, attestations gossiped under frame byte `0xA7`,
`consensus::bft::catchup` (fetch, verify, re-execute), the driver's
attested-follower mode and its rule for building again, `get_checkpoint`
and `get_bft_status` RPCs, and `maya2c-node --catch-up-from <URL>`.

One refinement over the decision above: a node resuming after a catch-up
puts its DAG horizon at the resume round itself, not `GC_DEPTH` below it,
because fetching ~50 rounds of parents over gossip lost replies under load.
And a follower builds again once its anchor is more than `GC_DEPTH` past the
resume round and its tip holds the previous anchor's block: with only three
of four validators up, two attesters cannot form a checkpoint to import.

- `bft_node_tests::a_validator_down_far_past_the_engine_window_rejoins_through_a_checkpoint`
  (held out 60 blocks, rejoins, keeps pace, agrees, proposes in current rounds).
- `bft_node_tests::every_member_holds_a_quorum_attested_checkpoint_on_its_own_chain`,
  `remote_signer_tests::a_remote_attestation_verifies_and_a_second_block_at_its_height_is_refused`,
  signer and `attest` unit tests.
- Real binaries, four local validators (`maya-rehearsal-1`): Dave stopped for
  more than 90 s, restarted with `--catch-up-from`, imported its gap, engine at
  round 432 against the network's 433, then built again from the anchor at
  round 408. Carol was then stopped: Alice, Bob and Dave went from height 186
  to 215 in 40 s, and all three hold block `4de4e92045f4593e` at height 213.
- Workspace: 3,082 tests passed, 0 failed.

Open: crossing a committee change during an outage (a checkpoint signed by a
different committee is refused, clearly), and an RPC source chosen
automatically instead of given by the operator.

## Crossing an epoch boundary (2026-10-05)

Found by `cargo xtask attacknet` (the long-outage attack). A validator that
was down across an epoch boundary could never rejoin. The network's newest
checkpoint was signed by the new committee, but the returning node trusts
only its own epoch's committee, so every checkpoint failed its quorum check.
Two more gaps sat behind that one:

- **Late attestations were discarded.** On a switch the driver dropped its
  collector and ignored attestations for the old epoch. So the epoch's last
  blocks, the boundary block among them, never got a checkpoint from the
  committee that ordered them.
- **A follower stayed in the old epoch.** A follower imports the boundary
  block rather than building it, so the epoch switch, which happened only
  on a built block, never fired.

As built:

1. The driver keeps the ended epoch's committee and collector (`Closing`)
   after the switch. Late attestations of that epoch complete its final
   checkpoint.
2. Each node publishes a `CheckpointBook`: the newest checkpoint of each of
   the last `KEPT_EPOCHS` (8) epochs. `get_checkpoint` takes an optional
   epoch, and the public gateways forward it. A validator down for longer
   than 8 epochs restores from a snapshot instead.
3. `catchup::fetch` asks for its own epoch's final checkpoint when the
   network is past that epoch. Importing up to it applies the boundary
   block, which writes the next committee into state. Startup catch-up
   repeats this until nothing is imported, one epoch at a time. Every
   checkpoint is still checked only against a committee that the node's own
   re-executed state names.
4. A follower switches epochs on the tick when its state has moved past the
   driver's epoch, then keeps following.

In the same run: startup catch-up no longer makes a node a follower when it
missed nothing. After an f+1 crash, three restarted followers had waited for
checkpoints that only they could complete.

Evidence: `cargo xtask attacknet`, all six attacks pass. Validator 6 was down
45 s across the boundary at height 60. It imported 33 blocks, logged
"followed attested blocks into epoch 1", and rejoined at height 66. There
was no fork through height 67. The run is in `reports/attacknet/`.
