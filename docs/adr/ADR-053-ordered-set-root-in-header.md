# ADR-053: Commit each anchor's ordered sub-DAG in the block header

**Status:** Proposed (for Eric). Not accepted. This changes the block format and
therefore consensus. It needs a spec update, conformance vectors
(`cargo xtask spec-coverage`), and a rust-reviewer and security-reviewer pass
before any code.
**Date:** 2026-10-18
**Relates to:** ADR-038 (checkpoints), ADR-052 (certificate replay), mainnet gate 10.

## Context

ADR-052 proposed that a follower re-derive blocks by replaying the epoch's
certificates. The measurement (ADR-052, "Measurement") put one epoch at
3,600 blocks at 50.8 s for n=4 and 65.8 s for n=7 in a debug build. That is over
the agreed 60 s gate for n=7, and it holds every certificate and every
signature of the epoch in memory (about 834 MB of signatures at n=7).

Replay costs O(epoch) because a block depends on the ordered set below the
resume point, and that set is not stored anywhere a follower can get it from
cheaply.

## Decision (proposed)

Each block header commits to the ordered sub-DAG of its anchor: a digest over
the ordered certificate digests of that anchor's causal history, in commit order.
A follower then needs only the certificates of the anchors in the gap, not the
whole epoch, and checks them against the committed digest.

## Why it is still safe

A block is canonical when the anchor that produced it has commit evidence the
committee signed: `n - f` valid signatures on the certificates that make the
anchor committed. The follower verifies those signatures itself with the
committee in its own executed state. The header digest then binds the sub-DAG,
so a peer cannot substitute a different causal history for the same anchor.
A peer that withholds certificates stalls the follower; it cannot make it
accept a different block.

## What it costs

- A consensus format change: header field, spec text, conformance vectors,
  and a height at which it activates (no silent change, invariant 24 applies).
- Every block carries a digest over its sub-DAG, so block verification gets
  more expensive. Measured: MISSING.
- Blocks already produced before activation cannot carry the field. Verification
  of the pre-activation range uses the existing checkpoint path.

## Open questions for Eric

1. Approve the format change and an activation height, or keep the whole-epoch
   replay and accept its memory and time cost (ADR-052 numbers).
2. Replay memory: even with this change, the verifying follower holds the gap's
   signatures at once. Streaming verification per anchor avoids that. It is a
   design detail for the implementation ADR.
3. Whether a checkpoint should also carry the anchor commit evidence, so a
   follower joining mid-epoch needs no certificates before the checkpoint.

## Not decided here

Whether the activation height is set by a governance vote or a release. That
is a policy decision.
