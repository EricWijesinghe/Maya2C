# ADR-048: `maya-amount` — one dependency-free 256-bit integer for the proved crates

- Status: Accepted and implemented (2026-10-13, branch `feat/u256-supply`), carrying out [ADR-046](ADR-046-universe-scale-supply.md)'s amendment (Eric: "Widen every amount to 256-bit")
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
  Added 2026-10-18: `long_division_step_keeps_the_invariant` proves one step
  of the restoring long division for every 256-bit divisor wider than 64
  bits and every remainder below it: the remainder stays below the divisor,
  and the step contributes exactly one correct quotient bit. Each step is
  checked against an independent five-limb adder. By induction, that covers
  the 512-step loop, which CBMC cannot unroll. Two seeded mutations were both
  caught: a dropped borrow in `sub`, and a wrong carry in `shl1_or`.

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

## Implementation record (2026-10-13)

What became 256-bit: transaction outputs (ENC-3/STF-2 in the spec), balances,
fees and fee splits, DEX reserves/shares/orders/prices/escrow (the order
book's storage key grows to 32+8 bytes), staking bonds/delegations/unbonding/
rewards/slashes, governance locks, deposits and vote tallies, HTLC escrow,
vault limits and withdrawals, payment-channel funding/balances/capacity and
the L2 channels and routing above them, RWA revenue and trade price, mining
rewards, the genesis config (allocations, treasury, bonds, minimum bond,
epoch reward), and the RPC and explorer representations (decimal strings).
Each stored record that changed width bumps its format version (staking 2,
vault 2, mining 3, HTLC lock 3) so an old record is refused, never misread.

Deliberately not 256-bit, each with its reason:

| Value | Width | Why |
|---|---|---|
| Nonces, heights, counts, ids | `u64` | Counters, not value. |
| Governed parameters (bps, limits, the proposal deposit) | `u64` | Each is a bounded rule; the deposit's own bound (≤ 10⁹) binds before the type does. The coin a deposit moves is 256-bit. |
| Per-byte base fee | `u64` | A rate. `rate × size` is computed exactly at 128 bits, then widened. |
| BFT voting weight | `u64` | The engine weighs in `u64`. Stakes above `u64::MAX` scale all weights by one common right-shift; below that the weights equal the stakes exactly, as before. |
| Shielded `public_in/out/fee` | `u64` | The joinsplit circuit range-checks them at 64 bits. The pool balance is 256-bit and settles through `ledger_math::settle_pool`. Shielded is off for mainnet v1. |
| RWA cap-table units | `u64` | A share count; the revenue split over them is 256-bit. |
| ISO 20022 bridge amounts | `u64` minor units | The message standard's amount. A chain output larger than it refuses the statement rather than clamping a line. |
| Faucet grants, mining-pool payouts | `u64` | App-internal figures; they widen at the chain boundary. |
| VM `get_balance` | `i64` | The original contract ABI, kept for existing modules: identical below 2^64, saturates above it. New host function `get_balance_u256(addr_ptr, out_ptr)` writes the exact 32-byte value. |
| GUI wallet (Leptos screens) | `u64` input | The core signs 256-bit amounts; the screens still enter u64. Follow-up. |

Proof scope, honestly: `maya-amount`'s harnesses prove add, subtract, order
and the byte encoding at every 256-bit input, the 512-bit product for factors
below 2^128, and division for divisors below 2^64; the long-division path is
property-tested, not proved. `ledger-math`, `staking` and `fee-market` harnesses take
256-bit symbolic inputs. Governance threshold harnesses are bounded to weights
below 2^128, and DEX pool harnesses to reserves below 2^40 (as before);
256-bit behaviour there is covered by tests, not proofs. Kani was NOT run in
this session — it runs nightly in CI.

The transaction wire format changed without a version bump: pre-genesis, and
any chain carrying the old 8-byte outputs is re-genesised (ADR-046).
