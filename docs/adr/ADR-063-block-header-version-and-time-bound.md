# ADR-063: A version byte and a timestamp bound in the block header

Status: Accepted (2026-10-27, Eric: version byte + median-time rule). Implementation also clamps the retarget span input so one far-future timestamp cannot steer difficulty; the clock-based drift check is an admission rule only, never part of `apply`. Originally Proposed 2026-10-26.

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

## As built (2026-10-27)

Three things differ from the draft above, each for a reason found while building.

1. **The version byte is appended, not inserted.** It is the 145th byte, after
   `tx_root`, so `NONCE_RANGE` (72..80) and `TX_ROOT_RANGE` did not move and no GPU
   kernel changed: the CUDA kernel implements only the Argon2 fill, and the header
   is hashed on the host at any length. The draft's "the miner kernel and
   `NONCE_RANGE` change" did not happen. A header with any version but 0 fails to
   decode (CON-10).
2. **The median rule applies only where the target is retargeted (CON-11).**
   The existing clock-drift rehearsal showed why: DAG-BFT blocks can come faster
   than one a second, and requiring each timestamp to exceed the median of the last
   eleven pushed block times ten seconds ahead of the clock in that run. A
   fixed-target chain has no retarget to steer, so it keeps its old rule (no
   earlier than its parent). The builder raises a candidate's timestamp to
   `median + 1` on a retargeting chain, so an honest miner whose clock lags is
   never refused.
3. **The retarget measures between median times, not between two timestamps.**
   This is what closes the steering: one block carrying an extreme timestamp moves
   neither end of the window, because it takes a majority of the eleven. The first
   window is five intervals short (genesis has no ancestors); every later window
   is exact for evenly spaced blocks. Tested by
   `one_far_future_timestamp_does_not_steer_the_retarget` and
   `a_window_of_evenly_spaced_blocks_measures_exactly_its_intervals`.

The span of eleven blocks is Bitcoin's and is a parameter of this ADR, not a
measured optimum. The clock-based bound from the draft is still not part of
consensus (Standing Order 4) and is not built.

Evidence: spec CON-1, CON-2, CON-5, CON-10, CON-11; `spec/tests/headers.json` and
`consensus.json` regenerated; the Rust node, the Rust reference (`spec-ref`) and
the TypeScript verifier all agree. The 144-byte ArgonBlake vector is unchanged and
still passes, showing the hash function did not change; a 145-byte vector was
added and is checked independently by the GPU-side hasher.

