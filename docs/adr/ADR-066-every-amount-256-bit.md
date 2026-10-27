# ADR-066: every amount is 256-bit behind the interface

- Status: Proposed (2026-10-27). Extends [ADR-046](ADR-046-universe-scale-supply.md).
- Scope: consensus-adjacent and pre-genesis. Needs `rust-reviewer` + `security-reviewer`
  before merge. Wire-format changes take effect only at the next re-genesis; nothing here is
  built into, or deployed to, the running testnet.

## Context

ADR-046 widened ledger balances to 256 bits. Around that core, narrower types remained:
a `u64` genesis supply, an `i64` contract-resource ABI, `u128` contract and route amounts,
and `u64` balances in the standalone application crates. Each one is a ceiling that another
system (Ethereum's `uint256`, a bridged token with a large supply) does not have, and each
conversion down to it is a place where a value can be clamped or wrapped.

## Decision

Every quantity of value — balance, supply, fee, allowance, escrow, limit, price paid — is
`maya_amount::Amount` (or the node's `U256`) end to end. Only the UI formats it down for
display. Conversions never clamp: a value that does not fit is an error, not `MAX`.

Applied in this change: genesis ceremony supply and treasury split; `contract-safety`;
the `maya_res` VM imports (an amount is four `i64` limbs, least significant first — WASM has
nothing wider); `treasury` and vesting; `smart-account`; `lanes`; `interop` routes and
intents; `clear-sign`; the `crypto-pq` migration; `l2-flash` barter; the permissioned-finance
CBDC vault; and the `reference-apps` and MCP server that sit on top.

## Not changed, and why

- Fixed external formats: Bitcoin satoshi `u64`, ISO 20022, Ethereum header RLP fields.
- Heights, nonces, epochs, byte counts, basis points and ppm ratios.
- MPC darkpool shares and SPDZ values: elements of a 64-bit field. Widening them is a
  protocol change, not a type change. `MAX_ORDER_SIZE` documents the bound.
- Shielded pool `public_in`/`public_out`/`fee`: STARK public inputs; a circuit change.
  Shielded transfers are off for mainnet v1.
- Base fee per byte (`fee-market`, `FeeRecord`), the oracle value and governance parameter
  values: planned next; each changes the wire format.
- Legacy `get_balance` (VM `env`): kept for already-deployed contracts; `get_balance_u256`
  gives the exact value.

## Consequences

- Transfers in `smart-account` are 48 bytes larger on the wire (amount and fee ceiling are
  32 bytes, not 8).
- A credit that would pass 2^256 − 1 is refused (`BalanceOverflow`, `OverCap`), where two
  paths used to saturate and silently destroy value.
- Contracts compiled against the old single-`i64` `maya_res` signatures must be rebuilt.
