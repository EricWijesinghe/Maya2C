# Prior art: Protocol-level scheduled and recurring payments

**Search status:** searched 2026-11-12 (web search; sources cited below).

Feature: v4 P-7, [ADR-135](../adr/ADR-135-v4-umbrella.md) §P-7, design text ADR-109 (PR #465), whose [scheduled-payments.md](scheduled-payments.md) is "partially searched". This file adds the 2026 native-rail precedent.

**Queries:** "native protocol-level scheduled recurring payments blockchain Substrate pallet-scheduler Internet Computer canister timers subscription".

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Solana Subscriptions and Allowances (2026-06-02) | A native on-chain program for recurring billing, capped agent allowances and contractor pulls; three models including a recurring delegation with a cap drawn down by the merchant. Pull-based: the payee draws. | [The Defiant](https://thedefiant.io/solana-ships-native-payments-rail-for-subscriptions-and-allowances), [solanatracker](https://www.solanatracker.io/blog/setting-up-subscriptions-and-recurring-payments-on-solana) |
| Base Spend Permissions | A documented pull mechanism on an EVM chain: the user grants a spend permission, the app pulls. | [Stripe overview](https://stripe.com/resources/more/recurring-crypto-payments) |
| Lightning BOLT 12 | Reusable offers with recurrence semantics; support uneven (CLN, LDK native; LND experimental). | [spark.money](https://www.spark.money/research/lightning-bolt12-recurring-subscription-payments) |
| Ethereum, Bitcoin | No native autopay; keepers (Chainlink Automation, Gelato) submit. | [Stripe overview](https://stripe.com/resources/more/recurring-crypto-payments) |
| Substrate `pallet-scheduler`, ICP canister timers | Not returned by this search; cited from the ADR-114 prior-art file (PR #468). | — |

## What Maya2C has (checked in repo, 2026-11-12)

- Height-driven settlement for vault requests (invariant 32), HTLC expiry,
  staking epochs; nothing that pays a user later.

## How the Maya2C version differs

- **Push, not pull**: the chain executes the due run at the end of the
  block from prepaid escrow; no payee action, no keeper, no relayer. Solana's
  rail and Base's permissions are pull-based delegations.
- Bounded per-block work (256 runs) with deterministic carry-over, under
  the undo journal and the state-root check (invariant 24).
- Block counts, never a clock (Standing Order 4).

**Verdict: similar exists (Solana native subscriptions, Base spend
permissions, BOLT 12). Differentiate by push execution from escrow with a
bounded end-of-block step. No "first" claim.**
