# Prior art: Contract timers and scheduled execution

**Search status:** partially searched 2026-11-12 (WebSearch, 2 queries: "Internet Computer canister timers Substrate pallet-scheduler Chainlink Automation scheduled contract execution"; "Substrate pallet-scheduler schedule named dispatch at block; Chainlink Automation time-based upkeep"). Chainlink Automation and Gelato returned no result and are **not searched**; listed from general knowledge.

Feature: parity row P-39, [ADR-114](../adr/ADR-114-contract-timers.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Internet Computer canister timers | The protocol gives each canister one global timer; the CDK builds multiple and periodic timers on it; delivery may be delayed under load; an upgrade clears pending timers; preferred over heartbeats. | [docs.internetcomputer.org timers](https://docs.internetcomputer.org/concepts/timers/), [DFINITY: canister timers](https://medium.com/dfinity/internet-computer-canister-timers-21cd3201b831), [ic-cdk-timers](https://docs.rs/ic-cdk-timers) |
| Substrate `pallet-scheduler` | Queues a runtime call for a block, named or anonymous, with optional period and priority; cancellable by id; a one-off task can be rescheduled only before it runs; origin filters from proxies are not kept at dispatch. | [pallet-scheduler docs](https://docs.rs/pallet-scheduler), [source](https://docs.rs/pallet-scheduler/47.0.0/src/pallet_scheduler/lib.rs.html) |
| Chainlink Automation, Gelato | Off-chain keepers call a contract on a time or condition trigger; the user prepays. **Not searched** this round. | — |

## What Maya2C has (checked in repo, 2026-11-12)

- Contracts run only on a signed `CallContract` (`crates/node/src/core/payload.rs:211-221`); `gas_limit` is a halting bound that bills nothing (`:216-220`); writes land only on success (`crates/node/src/state/vm_exec.rs:5-9`); no contract-to-contract calls; host ABI in `crates/vm/src/runtime.rs:661-964`.
- Height-driven end-of-block work exists for staking epochs and, proposed, for scheduled payments (ADR-109, `sp:` due index with a per-block cap).
- Standing Order 4: no wall clock; the height is the only time.

## How the Maya2C version differs

- **Scheduled from inside a call by a host function**, with the request a committed record under the state root (invariant 25); ICP's timers live in canister memory and are cleared by an upgrade, Maya2C's survive until run or cancelled.
- **Heights, never seconds**, and a bounded per-block run (64, carry-over in key order), in the shape of invariant 33 and ADR-109; Substrate also schedules by block but runs under a weight budget rather than a count.
- **Failure is a no-op**: a trapping timer cannot void the block (invariant 7's reason); a keeper-driven call that fails simply reverts for the keeper.
- **Flat prepaid fee per timer from a contract-owned escrow** anyone may fund, so a recurring timer pays its own next run; fuel itself is not priced on this chain, stated in the ADR.

**Verdict: similar exists (ICP timers, pallet-scheduler, keepers). Differentiate by committed timer records, the bounded key-order run, and no-op failure. No "first" claim.**
