# Prior art: Sponsored fees (a paymaster signs to pay another account's fee)

**Search status:** searched 2026-11-12 (web search; sources cited below).

Feature: parity row P-4, [ADR-106](../adr/ADR-106-sponsored-fees.md).

**Queries:** "ERC-4337 paymaster EIP-7702 sponsored transactions Sui sponsored transaction gas sponsor"; "Sui sponsored transactions GasData sponsor signature docs"; "Cosmos SDK x/feegrant fee allowance".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| ERC-4337 paymaster (Ethereum) | A contract that sponsors gas for smart-account user operations; the EntryPoint deducts from the paymaster's prefunded deposit; works only with smart accounts. EIP-7702 lets an EOA delegate to contract code and use the same sponsorship. | [Gelato: what is ERC-4337](https://docs.gelato.cloud/paymaster-&-bundler), [Coinbase paymaster](https://docs.cdp.coinbase.com/paymaster/introduction/welcome), [Para EIP-7702 sponsor gas](https://docs.getpara.com/v2/react/guides/web3-operations/evm/smart-accounts/eip-7702/sponsor-gas) |
| Sui sponsored transactions | `GasData` names the gas owner; when the owner differs from the sender the transaction is sponsored. Both the user and the sponsor sign over the entire `TransactionData` including `GasData`. Docs warn of equivocation risk and advise capped budgets and allow-listed functions. | [Sui docs](https://docs.sui.io/learn/sponsored-transactions), [Mysten SDK sponsor](https://sdk.mystenlabs.com/sponsor.md), [Sui blog](https://blog.sui.io/sponsored-transactions-explained) |
| Solana | Every transaction has a native fee-payer field; the fee payer signs. | [Eco comparison](https://eco.com/support/en/articles/15254045-gas-sponsorship-2026-how-apps-sponsor-user-fees) |
| Cosmos x/feegrant | One account grants another a fee allowance: basic (expiry, spend limit), periodic (limit per period), allowed-messages. One grant per granter/grantee pair; no self-grant. | [x/feegrant](https://docs.cosmos.network/sdk/latest/modules/feegrant/README.md), [Cosmos blog](https://blog.cosmos.network/secret-powers-what-are-the-authz-and-fee-grant-modules-c57d0e808794) |

## What Maya2C has (checked in repo, 2026-11-12)

- The fee is an ordinary signed output to `FEE_COLLECTOR`, never a field: `crates/node/src/state/fees.rs:5-11`. `FeeRecord::required(size)` at `fees.rs:92`.
- Every signature commits to the chain tag, the 32-byte genesis id (ADR-036; `crates/node/src/core/transaction.rs:66`, `signing_bytes` at `:299`).
- `crates/smart-account/src/fees.rs:98-111`: a RESEARCH `Paymaster { sponsored, per_account_daily, budget, spent }` with `charge`, not wired to the node.

## How the Maya2C version differs

- **Per-transaction, co-signed, like Sui**, not a contract deposit like 4337: the sponsor signs the sender's transaction id, a fee ceiling and an expiry height, under the chain tag. No contract, no bundler, no entry point.
- Unlike Cosmos feegrant there is no standing allowance record in state; the sponsor decides per transaction, so nothing can be drained by a grantee beyond one signed ceiling.
- The sender's frame is unchanged; sponsorship is an outer wire version, so stripping it just removes the fee and the frame is refused.

**Verdict: similar exists (Sui, Solana, 4337, feegrant). Differentiate by stateless per-transaction co-signing bound to the chain tag. No "first" claim.**
