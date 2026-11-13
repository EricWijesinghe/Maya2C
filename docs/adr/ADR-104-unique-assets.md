# ADR-104: Unique assets (NFTs): metadata hash only, royalties as an optional sale rule

**Status:** Proposed, 2026-11-12. Post-genesis (`postgen/v4`), dark until scheduled.
**Date:** 2026-11-12
**Related:** parity row P-2, [prior-art/nfts.md](../prior-art/nfts.md), [ADR-072](ADR-072-max-block-bytes.md), [ADR-103](ADR-103-native-fungible-tokens.md), [ADR-082](ADR-082-genesis-committed-upgrade-levers.md); invariants 7, 25, 28, 32, 33

## Context

No unique-asset record or transaction kind exists. The nearest shapes are
`AttestLegal` ("a hash, never a document") and `AnchorAttestation`
(`crates/node/src/core/payload.rs`), and the `contracts/nft-game` reference
app at the VM level. A block is capped at `MAX_BLOCK_BYTES` 5 MiB
(invariant 33, `crates/fee-market/src/limits.rs`) and a transaction at
`MAX_TX_BYTES` 4 MiB (`crates/node/src/core/transaction.rs:127`); a native
NFT that carried images would turn every mint into a byte-cap problem.
ERC-2981 signals royalties and does not enforce them (prior-art file).

## Decision

**A unique asset is a 32-byte content hash owned by an address, inside a
collection whose rules are fixed at creation. Royalties are an optional
collection rule settled by the chain on on-chain sales only.**

### State (invariant 25)

New prefix `n:` in a new committed layer `StateLayer::Nft` (next free tag
after PR #447's 18; assigned at implementation), added to `RECORD_LAYERS`
(`crates/node/src/state/commitments.rs:44`) so `uncovered_keys()` and the
`write_overlay` assertion cover it, and to the undo journal like every
overlay record.

- `n:c:` ‖ `collection_id(32)` → `Collection { creator, max_items: u64,
  minted: u64, royalty: Option<{ bps: u16, to: Address }>, sale_only: bool,
  metadata_hash: [u8; 32] }`. `collection_id = blake3::derive_key("maya2c nft
  collection v1", creator ‖ nonce)`.
- `n:i:` ‖ `collection_id` ‖ `index(8 BE)` → `Item { owner, metadata_hash:
  [u8; 32] }`.
- `n:o:` ‖ `owner` ‖ `collection_id` ‖ `index` → empty value; the owner index,
  so a wallet lists holdings by prefix scan without iterating a map (Standing
  Order 4 forbids `HashMap` order; this is a RocksDB prefix in key order).

**Metadata hash only.** The chain stores 32 bytes per item and 32 per
collection. Where the bytes live (IPFS, a web host, the owner's drive) and
what hash they are (BLAKE3 of the document is the convention) is the app's
business. A record never grows.

### Operations

| TxKind | Rule |
|---|---|
| `CreateCollection { max_items, royalty, sale_only, metadata_hash }` | sender is creator; `royalty.bps <= 10_000`; `max_items >= 1`. |
| `MintItem { collection, metadata_hash }` | sender == creator; `minted < max_items`; item index = `minted`; `minted += 1`. Owner is the creator. |
| `TransferItem { collection, index, to }` | sender == owner; refused if `sale_only`. No price, no royalty. |
| `SellItem { collection, index, buyer, price: U256 }` | co-signed by buyer and owner (the v8 multisig frame carries both approvals, `multisig_tx.rs`; or a sale offer record, see alternatives). Debit `price` from buyer; `royalty = price * bps / 10_000` to `royalty.to`; `price - royalty` to the owner; owner = buyer. |
| `BurnItem { collection, index }` | sender == owner; record deleted; `minted` does not fall. |

A `SellItem` whose buyer cannot pay is **refused**, not a no-op: a sale is an
authorization, not a trade that merely loses (invariant 7 applies to pools).

### 256-bit arithmetic

`price * bps / 10_000`: `price` is a native amount, `bps <= 10_000 < 2^14`.
The product fits 256 bits whenever `price < 2^242`; a `price` at or above that
is refused before the multiplication, with the bound written next to the
check. No 512-bit intermediate (Eric, 2026-11-01).

### What royalties cannot do

A `sale_only` collection still cannot stop a zero-price `SellItem` between
colluding parties, nor an off-chain payment beside a `TransferItem` when
`sale_only` is false. ERC-2981 has the same limit. The explorer and wallets
show the collection's rule and the sale price so a buyer can see what was
paid on chain.

## Alternatives considered

- **Blobs on chain under a byte limit.** Even 64 KiB per item makes a 5 MiB
  block hold 80 mints; invariant 33 would be the ceiling of every drop.
  Rejected.
- **NFTs only as WASM contracts.** Already possible (`contracts/nft-game`);
  the parity row asks for a native record the explorer and wallets can read
  without running contract code.
- **Royalty on every transfer.** A transfer has no price; the royalty would
  be a flat tax or zero. Rejected; royalties attach to sales.
- **Sale offer record instead of a co-signed sale.** An `OfferItem` record
  (`n:s:`) the buyer accepts later avoids coordinating two signatures but
  adds a record with an expiry and an undo path. Left to implementation;
  either shape satisfies this ADR.
- **ERC-1155 semi-fungibles.** Covered by ADR-103 assets with a small cap.

## Consequences

- Four new `TxKind` arms plus `SellItem`: `describe.rs`, `Module::of` (new
  `Module::Nft`, so a breaker halts mints and sales but never a native
  transfer, invariant 28), the vault door list (refused from a vault,
  invariant 32).
- One new layer, one new prefix family, three new state-layer decode tests.
- Explorer and wallet pages show collections, items and the royalty rule;
  the site follows in the same PR.
- Nothing is on at genesis.

## Invariants touched

- **25**: new prefix `n:` and layer; added to `RECORD_LAYERS`.
- **33**: the hash-only rule is the reason; no payload over 1 KiB.
- **28**: `Module::Nft`.
- **32**: NFT kinds refused from a vault.
- **7**: sale refusal stated.

## Activation

`FeatureSet::NFT = 1 << 12`, compiled `NFT_ACTIVATION_HEIGHT = u64::MAX`,
scheduled under ADR-082 with the v4 bundle at **genesis + 15,552,000**.

## Test plan

Red tests, `crates/node/tests/nft_tests.rs`:

- `the_root_is_unchanged_when_no_collection_exists`
- `a_mint_past_max_items_is_refused`
- `a_transfer_in_a_sale_only_collection_is_refused`
- `a_sale_pays_the_royalty_and_the_rest_to_the_owner_in_one_block`
- `a_sale_the_buyer_cannot_fund_is_refused_and_writes_nothing`
- `a_price_at_the_256_bit_bound_is_refused_before_multiplying`
- `a_reorg_restores_owner_and_minted_count`
- `an_nft_breaker_never_halts_a_native_transfer`

Vectors (`spec/03-state.md`, NF-1..NF-4): collection and item record bytes
and leaves; a sale with `bps = 250` on a stated price; a sale at
`bps = 10_000` (whole price to `royalty.to`).

## Open questions for Eric (economics and pricing only)

1. Collection creation anti-spam burn, and per-mint surcharge beyond bytes?
2. Is a protocol-level cap on `royalty.bps` wanted (NFT marketplaces commonly
   cap around 10%), or is 100% allowed as written?
3. Does the treasury take a cut of on-chain sales, or none?
