# Prior art: Scheduled and recurring payments executed by the chain at a height

**Search status:** partially searched 2026-11-12. Cosmos x/authz and x/feegrant sources below; Gelato Automate returned no result in one query and is **not searched**.

Feature: parity row P-7, CATALOG row `recurring-payments`, [ADR-109](../adr/ADR-109-scheduled-payments.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Cosmos x/authz | One account grants another the right to broadcast message types on its behalf, scoped by amount and time; a bot still submits each transaction. | [ADR-030 authz](https://docs.cosmos.network/sdk/latest/reference/architecture/adr-030-authz-module.md), [Akash authz guide](https://akash.network/docs/developers/deployment/authz/) |
| Cosmos x/feegrant periodic allowance | A per-period spend limit on fees (`--period 3600 --period-limit 10stake`), period in seconds. | [x/feegrant](https://docs.cosmos.network/sdk/latest/modules/feegrant/README.md) |
| Gelato Automate (Ethereum) | Off-chain keepers call a contract on a time or condition trigger; the user prepays. **Not searched** this round. | — |
| Ethereum / Solana | No protocol scheduling; keepers, clockwork-style programs or app servers submit on time. General knowledge, **not searched**. | — |

## What Maya2C has (checked in repo, 2026-11-12)

- No scheduled execution of user payments. End-of-block and end-of-epoch hooks exist: staking `end_epoch` (`crates/node/src/state/staking_exec.rs:312`), vault delayed requests settle by height (`crates/node/src/state/vault.rs`, invariant 32), HTLC expiry by height (`crates/htlc-lattice/src/timelock.rs`).
- `SessionScope { recipients, max_amount, expires_at }` in `crates/smart-account/src/policy.rs:41-47` could scope a relayer-driven subscription, which is the authz model.
- Standing Order 4: consensus has no wall clock; the block height is the only time.

## How the Maya2C version differs

- **The chain executes the payment** at the due height from a committed record; no keeper, no relayer, no grant to a third party. Cosmos authz and Gelato both need someone to submit.
- Periods are blocks, never seconds; a "monthly" schedule is `2,592,000` blocks at the one-block-per-second target, and the ADR says the calendar drift is the operator's to show, not the chain's to fix.
- Per-block work is bounded by a compiled constant with deterministic carry-over, in the spirit of first-fit under invariant 33.

**Verdict: similar exists (authz, feegrant periodic, keepers). Differentiate by protocol execution at a height with bounded per-block work. No "first" claim.**
