# Prior art: User-issued fungible tokens with a mint authority and a supply cap

**Search status:** searched 2026-11-12 (web search; sources cited below).

Feature: parity row P-1 (`.claude/plans/q1-90day-roadmap.plan.md`), [ADR-103](../adr/ADR-103-native-fungible-tokens.md).

**Queries:** "Solana SPL Token-2022 mint authority supply; Cosmos x/bank denom; ERC-20".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| ERC-20 (Ethereum) | A contract interface: `totalSupply`, `balanceOf`, `transfer`, `approve`/`allowance`. Minting, caps and decimals are whatever the contract code does; nothing is enforced by the chain. | [EIP-20](https://eips.ethereum.org/EIPS/eip-20) |
| SPL Token / Token-2022 (Solana) | A native program. The mint account holds `supply`, `decimals` and an optional `mint_authority`; with no authority the supply is fixed. The authority can be set only at creation per one source, and changed or revoked later per another (sources disagree). Extensions add close authority (only at zero supply), transfer fees, confidential balances. | [Token-2022 extensions](https://spl.solana.com/token-2022/extensions), [mint close authority](https://solana.com/docs/tokens/extensions/close-mint.md), [RareSkills](https://rareskills.io/post/token-2022) |
| Cosmos SDK x/bank | Module state: balances, denom metadata (base, display, denom units with exponents, symbol, URI) and total supply per denom. Minting goes through module accounts, not users. | [x/bank README](https://docs.cosmos.network/sdk/latest/modules/bank/README.md), [ADR-024 coin metadata](https://docs.cosmos.network/main/build/architecture/adr-024-coin-metadata) |

## What Maya2C has (checked in repo, 2026-11-12)

The gap table row "User-issued fungible tokens: none" in the roadmap plan is **wrong**. The tree already has:

- `TxKind::RegisterAsset(AssetRegistration)` and `TxKind::TransferAsset(AssetTransfer)`: `crates/node/src/core/payload.rs:457-460`, payload at `crates/node/src/core/dex_payload.rs:46-75`.
- `AssetRecord { creator, total_supply: U256, symbol: [u8; 8] }` under `d:asset:` in the Trading layer: `crates/node/src/state/asset.rs:52-66`, `crates/node/src/state/dex.rs:51`.
- A deliberate **no mint, no authority, no decimals** rule: `crates/node/src/state/asset.rs:17-33` ("a mint authority is an account that can dilute every holder, and introducing one is a governance decision, not a data-model one").
- `IssueRwa` for permissioned real-world assets: `payload.rs` (`IssueRwa`, `SettleDvp`, `RecordEligibility`).

What is missing against SPL/ERC-20 parity: a mint authority, a supply cap, mint and burn operations, authority renounce.

## How the Maya2C version differs

- The chain, not a contract, enforces the cap (contrast ERC-20). The cap is a `U256` compared with `checked_add`, no 512-bit intermediate.
- A mint authority is optional and renounceable, as in SPL; unlike SPL the cap is fixed at registration and can only fall (renounce freezes supply at the current total).
- No decimals field on chain (`asset.rs:27-33`); display is the wallet's business.
- The fee stays in the native coin as an output to `FEE_COLLECTOR` (`crates/node/src/state/fees.rs:5-11`). No fee-in-token at the protocol level.

**Verdict: similar exists everywhere. Differentiate by a chain-enforced immutable cap over the existing asset record. No "first" claim.**
