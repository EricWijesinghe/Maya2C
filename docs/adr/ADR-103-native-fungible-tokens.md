# ADR-103: Native fungible tokens with a mint authority and a 256-bit supply cap

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-1, [prior-art/fungible-tokens.md](../prior-art/fungible-tokens.md), [ADR-046](ADR-046-universe-scale-supply.md), [ADR-029 fee market via `fees.rs`], [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 25, 28, 33

## Context

The roadmap's parity matrix says "User-issued fungible tokens: none". That is
wrong, and saying so is part of this deliverable (Standing Order 9). The tree
has `TxKind::RegisterAsset` and `TxKind::TransferAsset`
(`crates/node/src/core/payload.rs:457-460`), an `AssetRecord { creator,
total_supply: U256, symbol: [u8; 8] }` under `d:asset:` in the Trading layer
(`crates/node/src/state/asset.rs:52-66`, `state/dex.rs:51`,
`state/commitments.rs:45`), balances per `(asset, holder)`, and DEX pools over
those assets.

What it lacks against ERC-20 and SPL is written down as a deliberate
limitation in `asset.rs:17-25`: no mint, no authority, supply fixed at
registration, because "a mint authority is an account that can dilute every
holder, and introducing one is a governance decision, not a data-model one".
This ADR is that decision. There is also no decimals field, by design
(`asset.rs:27-33`), and this ADR keeps that.

## Decision

**Extend the existing asset record with an optional mint authority and an
immutable supply cap; add mint, burn and renounce operations. The fee stays in
the native coin.**

### State (invariant 25)

No new prefix. `d:asset:` records gain a version 2 encoding:

```
v2: 0x02 ‖ creator(32) ‖ total_supply(32 LE) ‖ symbol(8) ‖ cap(32 LE) ‖ authority(32)
```

A v1 record (72 bytes, `ASSET_RECORD_LEN`, `asset.rs:48`) decodes as
`cap = total_supply`, `authority = [0; 32]` (none). Decoding is by length and
leading byte; a v1 record is never rewritten unless an operation touches it.
The record stays in `StateLayer::Trading`, so the root covers it and the undo
journal records prior values as today.

`authority == [0; 32]` means **no authority**; `[0; 32]` is also
`NATIVE_ASSET` and can own nothing (`asset.rs:14-15`), so no account can be
confused with "none".

### Operations

| TxKind | Rule |
|---|---|
| `RegisterAsset { symbol, total_supply, cap, authority }` | `total_supply <= cap`; `authority` is the sender or none; whole initial supply to the sender (unchanged). A v1-shaped registration (no cap, no authority) remains valid and means `cap = total_supply`. |
| `MintAsset { asset, to, amount }` | sender == authority; `total_supply.checked_add(amount)` must exist and be `<= cap`; credit `to`. |
| `BurnAsset { asset, amount }` | any holder burns its own units; `total_supply -= amount`. The cap does not fall; burned units may be re-minted up to the cap. |
| `RenounceMint { asset }` | sender == authority; sets `authority = none` **and `cap = total_supply`**, so the supply is frozen at exactly what exists. Irreversible. |
| `TransferAsset` | unchanged (`dex_payload.rs:59-70`). |

A mint that would pass the cap, a burn over the balance, a mint by a
non-authority: each is a refused transaction, not a no-op. Invariant 7's
no-op rule is for trades that lose; a mint that fails is an invalid
authorization.

### 256-bit arithmetic

`cap`, `total_supply`, `amount` are `U256` (ADR-046). Every operation is one
`checked_add` or `checked_sub` compared against `cap`. No multiplication, no
division, no 512-bit intermediate anywhere in this ADR.

### Fees

A token transaction pays like every other: an output to `FEE_COLLECTOR` in
the native coin (`crates/node/src/state/fees.rs:5-11`, `required(size)` at
`:92`). There is no fee-in-token at the protocol level; the smart-account
`FeeSpec::Token` (`crates/smart-account/src/fees.rs:28-33`) stays RESEARCH
and off this path. A sponsor may pay the native fee under ADR-106.

### Guard

`Module::of` already maps `RegisterAsset`/`TransferAsset` to a module; the
three new kinds map to the same one, so a breaker on that module halts
minting but never a native transfer (invariant 28).

### What stays out

Decimals (wallet's business, `asset.rs:27-33`), freeze authority, transfer
hooks, allowances (`approve`). An asset that needs those belongs in a
contract, as `asset.rs:23-25` says.

## Alternatives considered

- **New prefix `t:` and a Tokens layer.** Two homes for one kind of record;
  every DEX path would have to know both. Rejected.
- **Mutable cap raised by the authority.** A cap the authority can raise is a
  notice period, not a cap. Rejected; the cap only falls, at renounce.
- **Mint authority transferable to another account.** Deferred; it is one
  more arm and one more open question on who may hold it. Not in v4.
- **Tokens only as WASM contracts.** Already possible; the parity row asks for
  the native path, which the chain can bound and index.

## Consequences

- Existing assets are unaffected: v1 records decode to fixed supply with no
  authority. A chain that never mints has the same root it would have had.
- Wallets and the explorer show `cap`, `authority` and whether minting is
  renounced; the site's features page follows (feedback: site follows every
  upgrade).
- `AssetRegistration` grows two optional fields; the payload codec gains a
  trailing section so old registrations keep their shape.
- Three new `TxKind` arms: `describe.rs`, `Module::of`, the vault door list
  (invariant 32: token operations are not native transfers and are refused
  from a vault, like today's `TransferAsset`).

## Invariants touched

- **25**: record format change under an existing committed prefix; decode by
  version byte; `uncovered_keys()` unchanged.
- **7**: no new no-op; refusals stated.
- **28**: new kinds assigned to the existing module.
- **32**: new kinds listed as non-transfer doors.
- **33**: no new large payloads.

## Activation

`FeatureSet::ASSET_MINT = 1 << 11`, compiled `ASSET_MINT_ACTIVATION_HEIGHT =
u64::MAX`, scheduled by governance under ADR-082. Proposed with the v4 bundle
at **genesis + 15,552,000**. Before activation `MintAsset`, `BurnAsset`,
`RenounceMint` and a registration with cap or authority fields are refused on
admission and on apply.

## Test plan

Red tests, `crates/node/tests/asset_mint_tests.rs`:

- `a_v1_asset_record_decodes_as_capped_at_its_supply_with_no_authority`
- `a_mint_over_the_cap_is_refused_and_writes_nothing`
- `a_mint_by_a_non_authority_is_refused`
- `renounce_freezes_the_cap_at_the_current_supply`
- `a_burn_lowers_supply_but_not_the_cap`
- `a_mint_one_block_before_activation_is_refused_and_one_at_it_is_applied`
- `a_reorg_restores_supply_and_authority` (undo journal)
- `the_root_is_unchanged_on_a_chain_that_never_mints`

Vectors (`spec/03-state.md`, rules AS-1..AS-4): v1 and v2 record bytes and
their leaves; a mint at exactly the cap (accepted) and cap + 1 (refused).
`crates/node/tests/dex_tests.rs` keeps passing unchanged, as the regression
pin.

## Open questions for Eric (economics and pricing only)

1. Registration anti-spam: a fixed burn per `RegisterAsset` (to `FEE_SINK`)
   on top of the per-byte fee, and if so how much?
2. Does a `MintAsset` pay only bytes, or a surcharge proportional to units
   minted?
3. Should the treasury (ADR-068 tail reward, `crates/treasury`) receive any
   part of registration fees, or do they burn entirely?
