# Prior art: Account abstraction (session keys and spending limits enforced by the chain)

**Search status:** partially searched 2026-11-12. ERC-4337 and EIP-7702 sources from the sponsored-fees search; Starknet and Sui session-key material returned nothing in one query and is **not searched**. See also [native-smart-accounts.md](native-smart-accounts.md) (2026-09-26) and [personal-agent.md](personal-agent.md) (2026-11-10).

Feature: parity row P-5, [ADR-107](../adr/ADR-107-smart-account-promotion.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| ERC-4337 (Ethereum) | Smart-account user operations validated by account code through an EntryPoint and bundlers; session keys and limits are contract modules (e.g. Safe modules, ZeroDev). | [Gelato ERC-4337 overview](https://docs.gelato.cloud/paymaster-&-bundler) |
| EIP-7702 (Ethereum, Pectra, May 2025) | An EOA delegates to contract code at the same address. | [EF Pectra post](https://blog.ethereum.org/2025/04/23/pectra-mainnet) |
| Starknet, zkSync Era | Native account abstraction from genesis: every account is a contract that validates its own transactions. **Not searched** this round. | — |
| Sui | Sponsored transactions and zkLogin; session-key style scoping is app-level. **Not searched** this round. | — |

## What Maya2C has (checked in repo, 2026-11-12)

- `crates/smart-account` RESEARCH working: keys registered once, weighted threshold, `Policy`, `SessionScope { recipients, max_amount, expires_at }` with `allows(to, amount, height)` (`crates/smart-account/src/policy.rs:41-61`), `Action::AddSessionKey` (`op.rs:41`), bounded validation cost `MAX_VALIDATION_COST` 40 (`account.rs:17`). Not a node transaction type (`lib.rs:26-30`).
- PR #447 (`postgen/account-authority`, ADR-086 P3): wire version 9 account-authority frames, prefix `ka:`, layer tag 18, feature bit 9, policy digest and guardian recovery. Session keys and spending limits are **not** in it.
- Vault accounts enforce a per-window limit today (invariant 32, `crates/node/src/state/vault.rs`).

## How the Maya2C version differs

- Policies are **built-in modules evaluated by the chain**, not account code (`policy.rs:1`); validation cost is bounded by a compiled constant, so there is no gas market for validation.
- Keys are post-quantum suites from the registry (invariant 29); a session key is a suite key with a scope.
- Promotion reuses the v9 frame of PR #447 rather than a new wire version.

**Verdict: similar exists (4337, 7702, Starknet, zkSync). Differentiate by chain-evaluated policy modules under PQ suites. No "first" claim; Starknet and zkSync still to be searched before any comparison wording.**
