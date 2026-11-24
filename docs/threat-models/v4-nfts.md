# Threat model: unique assets (v4, P-2)

ADR: [ADR-135](../adr/ADR-135-v4-umbrella.md) §P-2 (design text in ADR-104, PR #465; `SellItem` and royalties cut to v5).
Written 2026-11-12. Tests: pending (build job I2).

## Attacker

The collection creator, any owner, a block producer, a marketplace that
reads `n:` records, and a reorg.

## Assets

- Ownership of an item (`n:i:` owner) and the owner index (`n:o:`), which
  wallets trust to list holdings.
- `minted` and `max_items` (scarcity).
- Block bytes (invariant 33): a mint is a permanent record paid once.

## Attacks

| # | Attack | Path |
|---|---|---|
| N-1 | **Ownership bypass**: transfer an item the sender does not own, or one in another collection with the same index. | Owner check keyed by `(collection, index)`. |
| N-2 | **Index resurrection**: burn item 7, mint again; if `minted` fell, the new item 7 inherits a stale `n:o:` row for the old owner. | `minted` never falls. |
| N-3 | **Owner-index desync**: a transfer rewrites `n:i:` but a crash or a bug leaves the old `n:o:` row; wallets list the item under two owners. | Both rows in one overlay write; reorg test covers both. |
| N-4 | **Mint past `max_items`** by overflow or by two mints in one block each seeing `minted = max - 1`. | Sequential apply within a block; `minted < max_items` per mint. |
| N-5 | **Collection-id grinding**: a creator picks a nonce so its id collides with or prefixes a known collection. | 32-byte BLAKE3 derive_key with a domain; a prefix cannot collide since keys are fixed width. |
| N-6 | **Royalty / sale bypass** (ADR-104's rule). | Cut: no sale on chain in v4; nothing to bypass. A "sale" is a transfer beside a payment; the wallet shows both. |
| N-7 | **Metadata swap**: the hash is immutable, but the off-chain document changes. | By design; hash-only (ADR-104). The explorer shows the hash; the app verifies the document. |
| N-8 | **Storage griefing**: a creator with `max_items = 2^32` mints billions of 65-byte rows. | Each mint pays its bytes; `MAX_COLLECTION_ITEMS = 2^32` is the bound; `CollectionCreationFee` (O-2) prices the permanent record. |
| N-9 | **Breaker bypass**: `Module::Nft` tripped, but transfers continue. | All four tags under `Module::Nft`. |
| N-10 | **Vault door**: an NFT transfer from a vault bypasses the window (no native amount moves, but value does). | Refused from a vault (invariant 32 non-transfer door), as `TransferAsset` is. |

## Mitigations

As the table's right column; all are checks in the apply path with the
same function at admission. Nothing here multiplies, so no arithmetic bound
exists to test beyond `minted` and `max_items` (`u64`, capped at `2^32`).

## Replay tests (`crates/node/tests/nft_tests.rs`)

1. `a_transfer_by_a_non_owner_is_refused`
2. `an_item_index_in_another_collection_does_not_authorise_a_transfer`
3. `a_burn_then_mint_never_reuses_an_index`
4. `a_transfer_moves_the_owner_index_row_in_the_same_write`
5. `two_mints_in_one_block_at_max_minus_one_apply_one_and_refuse_one`
6. `a_mint_in_a_full_collection_is_refused`
7. `a_reorg_restores_owner_minted_and_the_owner_index`
8. `an_nft_transfer_from_a_vault_is_refused`
9. `an_nft_breaker_halts_mint_and_transfer_but_not_a_native_transfer`
10. `the_root_is_unchanged_when_no_collection_exists`
11. Fuzz: `fuzz/` target over the four payload decoders (`cargo xtask
    proof-ratchet` counts it).
