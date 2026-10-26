# ADR-063: A version byte and a timestamp bound in the block header

Status: Proposed (2026-10-26). Needs Eric's decision. Header change: free before genesis, a hard fork after.

## Context
`BlockHeader` is a fixed 144 bytes (`core/block.rs`); the nonce range and the GPU
miner kernel depend on that layout. It has no version field, so any later header
change is a hard fork with an activation height. Invariant 9 records that a
block's timestamp has no consensus bound, so a miner can write any `u64`, which
is a difficulty-retarget manipulation surface on a proof-of-work chain.

## Decision (proposed)
1. Reserve a one-byte header version now, zero today, so a later header can be
   told apart without guessing from the length.
2. Bound the timestamp: strictly greater than the median of the previous blocks'
   timestamps, and not more than a fixed drift ahead of the validator's clock.
   The second half reads a clock, so it can only be an admission rule, not part
   of `apply` (CLAUDE.md Standing Order 4); the median rule is deterministic and
   consensus-safe.

## Consequences
The miner kernel and `NONCE_RANGE` change, the header grows by one byte, every
block id changes, and the spec and vectors are regenerated. All of it is free
only until genesis is frozen.
