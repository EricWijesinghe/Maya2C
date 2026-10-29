# ADR-067: 256-bit amounts inside the shielded pool and private state

- Status: **Proposed (2026-10-27). Needs Eric's decision and a cryptographic review
  before any circuit code is written.** Extends [ADR-066](ADR-066-every-amount-256-bit.md).
- Scope: protocol security. A change here alters what a STARK proof means.

## Context

ADR-066 widened every amount outside the proof systems. Two places remain narrower,
and both are narrower than they look:

| Where | Cap today | Why |
|---|---|---|
| Shielded pool note value, `public_in`, `public_out`, `fee` (`crates/zk-stark/src/pool`, `core/payload.rs`) | 2^58 − 1 (`MAX_VALUE`) | `BabyBear` is a 31-bit field; a value is two 29-bit limbs so every sum stays under the modulus and no equation can hold "mod p" without holding over the integers. The wire fields are `u64`, but a note cannot carry more than 2^58. |
| Private contract state (`crates/zk-stark/src/private_state.rs`) | 2^58 − 1 | The same two-limb design (`VALUE_BITS = 2 × LIMB_BITS`). |

At 18 decimals 2^58 base units is about 0.29 of a coin. So a shielded transfer or a
private field cannot hold even one whole coin. This is a smaller ceiling than the `u64`
the wire fields suggest, and it is not visible from the node's types.

Shielded transfers are off for mainnet v1 (ADR-037; the guard refuses a genesis that
enables them), so nothing on the launch path is affected.

## Proposal (not implemented)

Hold a value as `ceil(256 / 29) = 9` limbs of 29 bits, with an explicit carry or borrow
bit per limb, generalising what the two-limb design already does. Every term then stays
below 2^30, under the modulus, so the no-wraparound argument is unchanged.

Consequences that need design work, none of which a type change can deliver:

1. **Commitment width.** `C = P(lo, hi, 0.. ‖ r)[..8]` absorbs two limbs plus blinding in
   one width-16 permutation. Nine limbs plus blinding need a second absorption, so the
   note commitment, nullifier and Merkle-leaf hash all change.
2. **AIR size.** Value bits go from 58 to 261 columns per value, and the join-split
   carries several values, so trace width and proof size grow. **No figure exists yet;
   it is MISSING until a prototype is measured.**
3. **Public inputs.** `public_in`, `public_out` and `fee` become nine field elements
   each, and the verification-key and proof formats change.
4. **Soundness.** Range checks per limb, the carry chain, and the conservation equation
   need an independent cryptographic review. This is the part that must not be rushed.

## Decision needed from Eric

- Build it before shielded transfers are ever enabled, or keep shielded and private
  state off until a coin-sized value is needed? Either way the 2^58 ceiling should be
  stated wherever these features are described.
- If built: an external or independent review of the new AIR is required before any
  activation height is set, and no float, wall-clock or `unsafe` enters the path
  (Standing Order 4).

## Consequences of doing nothing

Shielded and private-state features stay capped at about 0.29 of a coin per value at 18
decimals. They remain off by the mainnet guard, so no funds are at risk; they are simply
unusable at coin scale.
