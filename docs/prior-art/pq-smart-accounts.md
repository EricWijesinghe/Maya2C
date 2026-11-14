# Prior art: Post-quantum smart accounts with session keys and spending limits

**Search status:** searched 2026-11-12 (web search; sources cited below).

Feature: v4 P-5, [ADR-135](../adr/ADR-135-v4-umbrella.md) §P-5, design text ADR-107 (PR #465). Extends [native-smart-accounts.md](native-smart-accounts.md) (partially searched 2026-09-26) and [account-abstraction.md](account-abstraction.md) (PR #465).

**Queries:** "post-quantum account abstraction ML-DSA smart account session keys blockchain 2026".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Sui | Announced native ML-DSA-65 accounts (testnet targeted by end of 2026) and SLH-DSA-SHA2-128s verification inside Move for high-value vaults. Native PQ accounts on an L1 with session-like capabilities through Move. | [The Block, 2026-08-06](https://theblock.co/news/web3/2026-08-06-sui-post-quantum-signature-scheme-410968) |
| Ethereum (Kohaku, EF PQ team) | ERC-4337 smart accounts that swap in quantum-resistant signature validators per account, opt-in; protocol readiness targeted around 2029. Session keys exist on 4337/7702 accounts today (thirdweb, ERC-7579 modules) with ECDSA. | [Etherspot digest](https://dev.to/etherspot/post-quantum-accounts-for-007-with-kohaku-gasless-ai-agent-payments-tokenized-treasury-on-3pc5), [Etherspot on EIP-7851 + quantum-safe MPC](https://etherspot.io/blog/glamsterdam-interop-milestones-clear-signing-launch-eip-7851-redesign-quantum-safe-mpc/) |
| Monad | Research proposal separating addresses from keys so accounts can rotate to passkeys or ML-DSA in place; notes that account abstraction "still cannot retire the underlying ECDSA key". | [Genfinity, 2026-08-25](https://genfinity.io/2026/08/25/monad-upgradeable-account-authentication-passkeys-post-quantum/) |
| PQSigAbstract (CU Boulder) | Modular PQ signature verifier for Ethereum compatible with ERC-4337 and RIP-7560; gas models for ML-DSA-44, Falcon-512, SLH-DSA-128f. Academic. | [scholar.colorado.edu](https://scholar.colorado.edu/concern/reports/05741t70f) |
| Cosmos SDK | Documentation for post-quantum key types in the SDK keyring. | [Cosmos docs](https://docs.cosmos.network/sdk/next/keys/post-quantum-keys) |

## What Maya2C has (checked in repo, 2026-11-12)

- `crates/smart-account`: keys with weights and threshold, `Policy`
  (daily limit, per-recipient, allow-list, delay), `SessionScope`, guardians,
  ML-DSA-65 in tests; RESEARCH, not a node transaction type.
- PR #447: wire v9 account-authority frames in the node, dark (bit 9).
- Every signature is a registry suite (ML-DSA, SLH-DSA, hybrid) under
  `check_admissible` (invariant 29); there is no ECDSA key to retire.

## How the Maya2C version differs

- Session scope and the daily window are **chain-evaluated rules on a
  native account**, not a contract module or a bundler-side validator; the
  signing keys are registry PQ suites from genesis, so there is no
  classical key underneath (the limitation Monad's proposal names).
- Sui's design is the closest: native PQ accounts on an L1. Maya2C's
  session keys are a scope on the same account (recipients, max amount,
  expiry, transfer-only) rather than Move capabilities.

**Verdict: similar exists (Sui native PQ accounts; 4337/7702 session keys
on classical keys; Kohaku opt-in PQ validators). Differentiate by
"PQ from genesis with chain-evaluated session scope". No "first" claim.**
