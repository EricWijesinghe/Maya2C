# Prior art: NFTs / unique assets with a metadata hash and optional royalties

**Search status:** searched 2026-11-12 (web search; sources cited below).

Feature: parity row P-2, [ADR-104](../adr/ADR-104-unique-assets.md).

**Queries:** "ERC-721 ERC-2981 royalties metadata hash"; "Solana Token-2022 extensions".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| ERC-721 / ERC-1155 (Ethereum) | Contract interfaces for unique and semi-fungible tokens; `tokenURI` points off chain; nothing about the content is committed on chain unless the contract chooses to. | [EIP-721](https://eips.ethereum.org/EIPS/eip-721) |
| ERC-2981 (Ethereum) | `royaltyInfo(tokenId, salePrice)` returns a receiver and amount. It **signals** royalties; it does not enforce payment. Detected via ERC-165 (`0x2a55205a`). | [OpenZeppelin ERC-2981](https://docs.openzeppelin.com/contracts/5.x/api/token/common) |
| Metaplex (Solana) | Token metadata program with a creators array and `seller_fee_basis_points`; enforcement historically depended on marketplaces, later "programmable NFTs" added transfer rules. | not fetched in this search; from general knowledge, marked **not searched** |
| Cosmos cw2981 | CosmWasm port of ERC-2981 royalty signalling. | [cw2981-royalties crate](https://lib.rs/crates/cw2981-royalties) |

## What Maya2C has (checked in repo, 2026-11-12)

- No NFT state or transaction kind. The closest shapes: `AttestLegal` ("a hash, never a document", `crates/node/src/core/payload.rs`) and `AnchorAttestation`.
- Block byte cap `MAX_BLOCK_BYTES` 5 MiB (invariant 33, `crates/fee-market/src/limits.rs`); transaction cap `MAX_TX_BYTES` 4 MiB (`crates/node/src/core/transaction.rs:127`).
- `contracts/nft-game` reference app exists (architecture-vision row "Reference applications"), VM-level, not a native asset.

## How the Maya2C version differs

- **Hash only, by rule.** An item carries a 32-byte content hash and nothing else; the chain never stores metadata bytes, so a collection cannot push a block toward the byte cap. ERC-721 leaves that to each contract.
- **Royalty enforced on on-chain sales only.** A `SellNft` settles coin and item atomically and pays the collection's royalty in the same transaction; a plain transfer carries no price and no royalty, exactly as ERC-2981 cannot see an off-chain payment. A collection may opt into `sale_only`, which routes every change of owner through a sale; a zero-price sale still evades the royalty, and the ADR says so.
- Native records under the state root (invariant 25), not contract storage.

**Verdict: similar exists. Differentiate by the hash-only rule and native royalty settlement on chain sales. No "first" claim.**
