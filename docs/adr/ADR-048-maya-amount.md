# ADR-048: `maya-amount` — one dependency-free 256-bit integer for the proved crates

- Status: Proposed (2026-10-13), to carry out [ADR-046](ADR-046-universe-scale-supply.md)'s amendment
- Scope: consensus-critical, pre-genesis only. `rust-reviewer` + `security-reviewer`, Kani proofs in
  the crate, conformance vectors through the spec work of ADR-046.

## Context

ADR-046, amended 2026-10-13, widens every amount to 256 bits. The node uses
`primitive-types::U256`. Five crates hold money arithmetic and are kept free of
external dependencies so Kani can compile and prove them whole: `ledger-math`
(invariant 1), `dex` (invariant 6), `fee-market`, `staking` and `governance`.
All are `no_std` with an empty `[dependencies]`.

They cannot take `primitive-types`. That adds `uint`, `fixed-hash`, `crunchy`,
`static_assertions` and optional serde/rlp to the graph Kani must compile, and it
breaks the rule those invariants exist to keep. Writing a private 256-bit type
in each crate would mean five copies of carry, borrow and comparison code, each
needing its own proofs, and five places for one bug to hide.

The DEX also needs more than 256 bits in the middle of a calculation: a
constant-product quote multiplies two 256-bit reserves.

## Decision

A new workspace crate, **`crates/amount` (`maya-amount`)**:

- `no_std` unconditionally, **zero dependencies**, no `[features]`, the same
  manifest shape as `ledger-math`.
- `Amount([u64; 4])`, limbs little-endian, with `ZERO`, `MAX = 2^256 − 1`,
  `From<u64>`, `checked_add`, `checked_sub` and `checked_mul`, a widening
  `mul_wide` to a 512-bit value with a `checked_div` back to `Amount` (for
  `x·y/z`), a total `Ord`, and `to_le_bytes`/`from_le_bytes` on `[u8; 32]`,
  which is the same wire encoding as the node's `U256`.
- Kani harnesses in the crate: no checked operation wraps, add and subtract are
  inverses, the order matches the integer order, and `mul_wide` then
  `checked_div` returns the exact quotient.

`ledger-math`, `dex`, `fee-market`, `staking` and `governance` depend on it by
path. That is a dependency on a crate with no dependencies, so each one's Kani
graph stays all first-party, `no_std` Rust. The invariants' reason holds. Their
letter is updated in `docs/invariants.md` from "no dependencies" to "no
dependency outside the workspace's proved crates".

The node converts at the crate boundary. `U256` and `Amount` share the
little-endian 32-byte encoding, so the conversion is a byte copy that is
tested both ways.

`crates/spec-ref` does **not** use `maya-amount`. The reference keeps its own
256-bit type, so a bug in the shared integer cannot pass the conformance
vectors by being present on both sides.

## Consequences

- One place to prove 256-bit arithmetic, instead of five.
- Five invariant texts change wording, not intent. Each change is reviewed.
- Two 256-bit types exist on the node side (`U256` in the node, `Amount` in
  the proved crates). Mixing them up is a compile error, not a silent bug.
