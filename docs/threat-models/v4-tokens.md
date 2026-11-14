# Threat model: token mint and burn (v4, P-1)

ADR: [ADR-135](../adr/ADR-135-v4-umbrella.md) §P-1 (design text in ADR-103, PR #465).
Written 2026-11-12. Tests: pending (build job I1).

## Attacker

The mint authority itself (malicious issuer), any holder, a block producer,
and a reorg. The authority is trusted by holders only to the extent of the
cap; everything else assumes it is hostile.

## Assets

- Holders' share of supply (dilution).
- `total_supply`, `cap`, `authority` in the `d:asset:` record (the state
  root, invariant 25).
- The native coin (no path from a token to native balance exists; must stay so).
- DEX pools quoting the asset (a supply change moves nothing in a pool,
  but a holder's expectation of scarcity).

## Attacks

| # | Attack | Path |
|---|---|---|
| T-1 | **Mint over the cap** by overflow: `total_supply + amount` wraps past 2^256 and lands under `cap`. | Arithmetic. |
| T-2 | **Cap raise by re-registration**: register a second asset with the same symbol and a higher cap, trade on the name. | Symbol is not unique; id is `creator ‖ nonce ‖ symbol`. |
| T-3 | **Renounce then mint**: renounce in one block, mint in a later one via a reorg that drops the renounce. | Undo journal must restore `authority`. |
| T-4 | **Mint by a non-authority** via a v8 multisig whose policy includes the authority key at weight below threshold. | Sender address check. |
| T-5 | **Zero-amount mint spam** to rewrite v1 records as v2 and churn the root for free. | `amount = 0`. |
| T-6 | **Burn another holder's units** by naming an asset the sender does not hold (balance underflow). | `checked_sub` on the sender's balance only. |
| T-7 | **Cap below supply at registration** so the record is internally inconsistent and a later burn/mint sequence overflows `cap - supply`. | `total_supply <= cap` at registration. |
| T-8 | **Authority = native asset** (`[0;32]`) set deliberately to look "renounced" while a key for the zero address... | No key hashes to `[0;32]` (BLAKE3 preimage); `[0;32]` is "none" by definition. |
| T-9 | **Pre-activation mint** included by a producer running a v4 binary before `V4_ACTIVATION`. | Both admission and apply refuse by height. |
| T-10 | **Breaker bypass**: `Module::Dex` tripped, but `MintAsset` is routed to no module. | `Module::of` arm for every new tag. |

## Mitigations (as designed)

- T-1: `checked_add` then `<= cap`; vector at `cap` / `cap + 1`.
- T-2: Accepted, as today for `RegisterAsset`: symbols are not unique on
  purpose (`asset.rs`); the wallet shows the asset id and creator. No new
  exposure.
- T-3: `d:asset:` is an overlay record under `capture_undo`; reorg test.
- T-4: `sender == authority` compares the frame's sender address, which for
  a multisig is the policy address, not a member key.
- T-5: `amount > 0` required.
- T-6: Balance `checked_sub` on `(asset, sender)` only.
- T-7: Refused at registration.
- T-9: Height check in one function called by both paths.
- T-10: Exhaustive `match` in `Module::of`; the compiler enforces it.

## Replay tests (build lane, `crates/node/tests/asset_mint_tests.rs`)

1. `a_mint_whose_sum_wraps_past_u256_max_is_refused`
2. `a_mint_at_exactly_the_cap_is_applied_and_one_over_is_refused`
3. `a_reorg_that_drops_a_renounce_restores_the_authority_and_cap`
4. `a_multisig_member_below_threshold_cannot_mint_as_the_authority`
5. `a_zero_amount_mint_is_refused_and_leaves_the_v1_record_untouched`
6. `a_burn_over_the_senders_balance_is_refused_even_if_supply_covers_it`
7. `a_registration_with_cap_below_supply_is_refused`
8. `a_mint_one_block_before_v4_activation_is_refused_and_at_it_is_applied`
9. `a_dex_breaker_halts_mint_and_burn`
10. `the_root_is_unchanged_on_a_chain_that_never_mints` (regression pin for
    `dex_tests.rs`)
11. Mutation check: remove the `<= cap` comparison and record which of 1-10
    fail (expect 2).
