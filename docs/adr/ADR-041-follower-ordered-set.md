# ADR-041: Followers become builders after one checkpoint (gate 10)

**Status:** Proposed. It needs Eric's approval because it changes the
attestation wire format: every node upgrades together, as for any
consensus-adjacent change.

## Context

A node down past the engine's window (`GC_DEPTH` = 50 rounds) catches up
through an n − f checkpoint, then **follows** (ADR-038). It votes, but it
imports blocks instead of building them.

It cannot build because it does not know which vertices earlier anchors
already ordered. The engine's `Committer.ordered` set holds that knowledge
(round, digest pairs within the GC window), and a restarted node lost it.
So it waits until it has seen `GC_DEPTH` rounds itself
(`may_build_again`).

A follower also cannot attest a block it has not imported. When more than f
validators follow at once, the builders alone are fewer than n − f, so no
checkpoint ever forms and the followers never import. Two things happen:

- The chain keeps running, because followers still vote.
- The followers are stranded until they are restored from a snapshot.

The attacknet soak reproduced this on 2026-10-05. Eric kept the n − f
threshold, which rules out lowering the checkpoint quorum as a fix.

## Decision (proposed)

Make a follower able to build **right after its first checkpointed import**,
by certifying the ordered set alongside the block:

1. **The attestation signs one more thing.** Each attestation signs
   `ordered_root`: a hash of the attesting builder's `Committer.ordered` set
   right after it built the block (sorted `(round, digest)` pairs, so it is
   deterministic). Honest builders of the same block hold the same set, so
   an n − f checkpoint now certifies the block and its ordered set
   together.
2. **A new RPC, `get_ordered_set(height)`.** It returns the pairs, held for
   the heights in the `CheckpointBook`. The set is bounded: at most
   n × `GC_DEPTH` entries, about 11 KB at n = 7.
3. **Catch-up seeds the engine.** After importing up to a checkpoint, a node
   fetches that height's ordered set from its peer. It checks the set
   against the checkpoint's `ordered_root` and refuses it on a mismatch. It
   then seeds `Committer.ordered` with it and resumes as a **builder**, not
   a follower.
4. **`may_build_again` remains only as the fallback** for a node whose peer
   cannot serve the set.

## Why it holds

- **Trust stays at n − f.** The ordered set is accepted only when it hashes
  to a root that n − f of the committee signed. The node already trusts the
  block on exactly that quorum.
- **Followers last seconds, not 50 rounds.** So more than f of them at once
  stops being a stable state, and the deadlock goes away without changing
  any threshold.
- **No block-format change.** The block header is untouched. Only the
  attestation digest and frame grow, by one 32-byte root.

## Costs and risks

- **Attestation format v2.** v1 and v2 nodes cannot count each other's
  attestations. That is fine on a testnet that upgrades together, and is
  part of the mainnet genesis format.
- **Ordered-set determinism.** Every honest builder must hold an identical
  set after the same block. GC pruning has to be tied to the anchor round,
  not to wall time. This needs a test across restarts. It is the core risk.
- **Spec work.** A spec update and conformance vectors are needed for the
  attestation digest.

## Evidence required before acceptance

- Unit tests:
  - two engines fed the same certificates produce the same `ordered_root`;
  - a restarted engine seeded from the set builds byte-identical blocks.
- The attacknet soak (5 × 3 rounds) passes, with a new attack: f + 1
  validators down past the window at once.
- A security review of the seeding path, in particular a peer serving a set
  that matches the root but is stale. The root binds the height, so such a
  set is refused.
