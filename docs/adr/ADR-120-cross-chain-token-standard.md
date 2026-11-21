# ADR-120: Cross-chain tokens — a route as a rate-limited mint authority

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled. Depends on ADR-103 and ADR-122.
**Date:** 2026-11-12
**Related:** parity row P-23, [prior-art/cross-chain-tokens.md](../prior-art/cross-chain-tokens.md), [ADR-103](ADR-103-native-fungible-tokens.md), [ADR-065](ADR-065-cross-chain-asset-routes.md), [ADR-122](ADR-122-cross-chain-messaging.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 11, 25, 28, 31, 32

## Context

An asset today is `AssetRecord { creator, total_supply, symbol }` under
`d:asset:`, supply fixed at registration, no mint
(`crates/node/src/state/asset.rs:17-25`, `:56-63`); ADR-103 adds an optional
single `authority` and a `cap`, with `MintAsset` allowed only to that
authority. `crates/interop::routes` caps value per route per window and
grows the cap with clean days (`routes.rs:3-8`, `:65-93`). ADR-065 chose
swaps first, then Bitcoin lock/mint behind `u64::MAX` with disclosed custody
and a per-epoch mint cap, then Ethereum receipt proofs (ADR-122).

**Finding on the brief.** The row says "burn/mint authority over P-13
routes". There are no P-13 routes in consensus: `crates/interop::routes` is a
library nothing calls (`features.toml:3291` RESEARCH), and ADR-065's routes
are an accepted plan with an activation height nobody has written. "Over
P-13 routes" therefore means "over ADR-122's inbound messages and ADR-065's
Bitcoin deposit proofs, whichever exists", and this ADR cannot activate
before one of them does. The row's dependency "after P-1, P-13" is right;
P-13 is further away than the row's size `M` suggests.

xERC20 (ERC-7281) lets a token issuer allow several bridges, each with its
own mint and burn rate limit, so a compromised bridge is bounded by its
allocation; Chainlink's CCT does the same with per-lane token-bucket limits;
LayerZero's OFT ties the token to one verifier set (prior-art file).

## Decision

**An asset's authority (ADR-103) may delegate minting to one or more
*routes*, each with its own per-window cap; a route mints only against a
proven inbound event (ADR-122 message or ADR-065 deposit) and burns only on
an explicit holder transaction that records the destination. The asset's
cap (ADR-103) still bounds total supply; the route cap bounds what one route
can mint per window; both are 256-bit checked arithmetic.**

### State (invariant 25)

Under the existing asset record (no new prefix for the asset) and the bridge
layer `x:`:

- `x:tok:route:` ‖ `asset_id(32)` ‖ `route_id(32)` → `TokenRoute { cap:
  Route (routes.rs:14), minted_window: U256, burned_window: U256, paused:
  bool }`. `route_id` is `blake3::derive_key("maya2c token route v1",
  src_chain ‖ src_contract)` for an ADR-122 source, or the ADR-065 route
  identifier for Bitcoin.
- `x:tok:foreign:` ‖ `src_chain(8 BE)` ‖ `src_token(32, left-padded)` →
  `asset_id`: the canonical mapping, written once, so a foreign token has one
  Maya2C asset and not one per bridge (the fungibility problem ERC-7281
  names).
- `x:tok:out:` ‖ `nonce(8 BE)` → `{ asset_id, amount: U256, dst_chain,
  dst_address(32), burned_at }`: the burn record, exported as ADR-122's
  outbound record is (and with the same caveat: nothing on the other side
  verifies it yet).

### Operations

| TxKind | Rule |
|---|---|
| `SetTokenRoute { asset, route, cap }` | sender == ADR-103 `authority`; writes or replaces the route record; `cap.ceiling <= asset.cap - asset.total_supply` at the time of writing (a route may never be allowed more than the headroom). A `cap.ceiling` of zero pauses. |
| `RegisterForeignToken { src_chain, src_token, symbol, cap }` | Creates the asset with `total_supply = 0`, `authority` = the derived route address, and the `x:tok:foreign:` mapping; refused if the mapping exists. Anyone may call; the first registration wins and the symbol is informational (ADR-103's rule). |
| Mint (no new kind) | Performed by ADR-122 delivery or ADR-065 deposit apply when the message's `target` is the asset's route address and the payload decodes as `TokenMint { to, amount }`. Checks: route exists and not paused; `minted_window + amount <= cap(height)` (`Route::send`, `routes.rs:77`); `total_supply + amount <= asset.cap` (ADR-103). A failed check is a **refused delivery that waits** (the nonce does not advance), so a cap is a rate limit, not a loss. |
| `BurnToSend { asset, amount, route, dst_address }` | Holder burns its own units (ADR-103 `BurnAsset` rule) and writes `x:tok:out:`; `burned_window + amount <= cap(height)`, so a burn cannot outrun the route either (ERC-7281's burn limit). |

### Incidents

`TokenRoute.paused` is set by the asset authority (`SetTokenRoute` with
ceiling zero) or by a governance proposal (ADR-111's lifecycle) naming the
route; `Route::incident` (`routes.rs:95`) resets the clean clock so a resumed
route starts from `base` again. There is no automatic incident: the chain
cannot know a mint was wrong, only that it was proven.

## Alternatives considered

- **One global mint authority = the bridge (OFT).** A single bridge bug
  mints to the asset cap. Per-route caps are why ERC-7281 exists.
- **Mint on `CallContract` from a relayer.** Trusts the relayer (invariant
  11's reason). Mint only against a proven event.
- **Lock/unlock instead of burn/mint for Maya2C-native assets.** The locked
  balance would sit in a derived account that no proof can release without
  the outbound verifier that does not exist. Burn/mint keeps supply honest
  on this side today; the record is what the other side will one day verify.
- **Drop a delivery that exceeds the cap.** Loses the user's funds on the
  other chain. It waits.
- **Per-bridge wrapped assets (`USDC.bridgeA`, `USDC.bridgeB`).** Exactly
  the fungibility failure the foreign mapping prevents.

## Consequences

- Foreign tokens arrive as native ADR-103 assets with a published route and
  a published cap; every wallet string says which route minted and that it
  "inherits the source chain's signature security" (ADR-122).
- Three new kinds, one mint path inside two existing apply paths, three
  prefix families under `x:`.
- Supply accounting stays in the one place ADR-103 put it; the invariant
  guard's conservation check gains `minted − burned` per route per window.
- A route cap too low stalls deliveries by design; the authority raises it
  or governance does.

## Invariants touched

- **11**: no trusted party; mint only against ADR-122/ADR-065 proofs.
- **25**: `x:tok:` records in the bridge layer; undo coverage.
- **7**: refused route writes are errors; a capped delivery waits.
- **28**: `Module::Bridge` (shared with ADR-122) gates every arm here.
- **31**: no wire change; v6 payload kinds.
- **32**: `BurnToSend` is a vault door that counts toward the window like a
  transfer; `SetTokenRoute` and `RegisterForeignToken` move no value.
- ADR-066: all amounts `U256`, `checked_add`, no 512-bit intermediates.

## Activation

`FeatureSet::XCHAIN_TOKEN = 1 << 27` (claimed in `adr-claims.txt`), compiled
`XCHAIN_TOKEN_ACTIVATION_HEIGHT = u64::MAX`, scheduled under ADR-082 with the
v4 bundle at **genesis + 15,552,000**, and never before bit 11
(`ASSET_MINT`) and whichever of bit 29 (`XCHAIN_MESSAGING`) or ADR-065 option
2 is live. Before activation every kind here is refused and a delivery to a
route address is a receipt only.

## Test plan

Red tests, `crates/node/tests/xchain_token_tests.rs`:

- `a_proven_mint_within_both_caps_credits_the_holder_and_raises_supply`
- `a_mint_over_the_route_window_waits_and_the_nonce_does_not_advance`
- `a_mint_over_the_asset_cap_waits`
- `a_route_ceiling_above_headroom_is_refused_at_set`
- `a_second_registration_of_one_foreign_token_is_refused`
- `burn_to_send_writes_the_out_record_and_counts_against_the_burn_window`
- `a_paused_route_mints_nothing_and_resumes_from_base`
- `conservation_holds_with_minted_minus_burned_per_route` (guard)
- `a_vault_burn_to_send_counts_toward_the_window` (invariant 32)
- `a_reorg_restores_route_windows_and_supply`

Vectors (`spec/interop/`): a `TokenRoute` record; one mint; one burn record.
`cargo xtask spec-coverage` lists them.

## Open questions for Eric (economics only)

1. Default route cap shape at registration (`base`, `per_clean_day`,
   `ceiling` as a fraction of the asset cap), and whether a foreign token's
   first route cap is set by the registrant or by governance.
2. Does a mint pay a fee into the treasury (ADR-111) per unit, per event, or
   nothing beyond the deliverer's bytes?
3. Should `RegisterForeignToken` carry a deposit to stop symbol squatting,
   given the symbol is informational?
