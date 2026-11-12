# ADR-139: Intent solver market settled over the existing hash-lock HTLC

**Status:** Proposed, 2026-11-12. Off chain; no consensus change.
**Date:** 2026-11-12
**Related:** parity row P-30 (P-29 merged into it), [ADR-012](ADR-012-hash-lock-htlcs.md) (hash-lock HTLC), [ADR-084](ADR-084-mcp-read-and-draft-only.md) (MCP server), `crates/interop/src/intents.rs` (proof-settled escrow, RESEARCH); invariants 9, 11, 31; Standing Orders 4, 6

ADR number 139 was claimed in `adr-claims.txt` beside 135-138 from parallel
branches; the coordinator may renumber.

## Context

A maker who wants "7 units of A for at most 500 of B" today has to find a
counterparty and run an HTLC swap by hand. Solver markets (NEAR Intents, CoW,
Skip Go; `docs/ecosystem/parity-matrix.md`) put a competitive quote step in
front of settlement. The chain already has the settlement half: `HtlcLock`,
`HtlcClaim` and `HtlcRefund` with SHA3-256, BLAKE3 and SHA-256 hash locks,
live from genesis (`crates/node/src/core/htlc_payload.rs`,
`crates/node/src/state/htlc_exec.rs`). The brief forbids a new consensus form.

## Decision

A crate, `crates/intent-solver`, and four MCP tools, all off chain.

1. **Intent format.** Network tag, maker, sell asset and `max_sell`, buy
   asset and exact `buy_amount`, buy recipient (≤ 64 bytes), deadline height,
   nonce. Canonical fixed-width encoding, signed under a domain string with
   the hybrid ML-DSA-65 + SLH-DSA-SHA2-128s pair (suite `0x30`, both halves
   must verify). The maker is `suite_address(0x30, public_key)`.
2. **Book.** Refuses a malformed intent, a wrong network, a key that is not
   the maker's, a bad signature, an intent at or past its deadline height, and
   any second intent on a spent `(maker, nonce)`. Spent nonces outlive pruned
   intents.
3. **Quotes.** `price + fee` (checked `U256`) must not exceed `max_sell`.
   One quote per solver; a solver can only lower its own quote, because quotes
   are not signed and a raise in its name could come from anyone. Ranking is
   `(total, solver id)` over a `BTreeMap`, a total order independent of arrival.
4. **Settlement.** The maker holds the preimage, so the maker locks first with
   expiry = deadline; the solver locks second with expiry = deadline − margin;
   both locks must land before `lock_by` = solver expiry − margin. The maker's
   claim reveals the preimage; the solver has at least one margin to claim.
   Each lock refunds to its sender from its own expiry. The tracker applies
   `maya_htlc_lattice::decide_claim`/`decide_refund`, the node's own rule.
5. **MCP.** `intent_post`, `intent_offer`, `intent_quotes`,
   `intent_settle_plan`. Heights come from the node. No tool signs or submits.

## Limits written down

- **`HtlcLock` escrows the native coin only.** So at most one leg settles on
  Maya2C; the other is an HTLC on the asset's own chain under the same digest
  (`bins/htlc-watcher`). A plan with a non-native leg requires SHA-256, the
  hash other chains can check. The non-native leg's expiry is stated in Maya2C
  heights; converting it is the watcher's pairing policy, not checked here.
- **Quotes are unsigned.** A forged quote can win and make the maker lock to a
  solver who never locks back. The maker loses no funds (refund at the
  deadline) but loses time. Signed quotes are the next step if griefing shows.
- **The book is in memory** in one MCP process, with fixed caps
  (`MAX_INTENTS`, `MAX_SPENT_NONCES`, `MAX_QUOTES_PER_INTENT`) that are chosen,
  not measured. A replay is refused only by the book that saw the first post;
  the chain never sees an intent, so two books do not share nonces.
- **No unsigned transaction drafting** for the lock/claim/refund steps yet; the
  plan gives the fields a wallet needs.
- Confidential intents (the "optional" half of P-30) are not built.

## Consequences

No state prefix, no wire change, no activation height: invariants 24-25 and
31 are untouched. Invariant 7 is respected by construction because losing a
quote is an off-chain refusal, and losing an HTLC race is already a no-op on
chain. Promotion past `stub` needs CI green, a red-team pass on the book, and
a two-chain swap test through `htlc-watcher`.
