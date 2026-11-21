# Prior art: Vote delegation and conviction voting

**Search status:** searched 2026-11-12 (WebSearch, 2 queries: "Cardano CIP-1694 DRep delegation Polkadot OpenGov conviction voting lock multiplier"; "Polkadot OpenGov treasury tracks small spender medium spender conviction voting wiki").

Feature: parity row P-27, [ADR-113](../adr/ADR-113-vote-delegation.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Cardano DReps (CIP-1694) | Stake credentials delegate to a registered DRep (or a predefined option); one lovelace, one vote; one hop; DRep incentives unresolved at the time of the sources; CIP-0181 removed the rule that blocked reward withdrawals for undelegated stake. | [Intersect: CIP-1694 explained](https://www.intersectmbo.org/news/cardano-cip-1694-explained), [CIP-1694](https://cips.cardano.org/cip/CIP-1694), [CIP-0181](https://cips.cardano.org/cip/CIP-0181), [developers.cardano.org governance](https://developers.cardano.org/docs/developers/curriculum/staking-governance/governance/) |
| Polkadot OpenGov conviction voting | Votes = tokens x conviction multiplier; the multiplier rises by one each time the lock period doubles (28-day periods), up to a maximum; approval is conviction-weighted, support is measured against issuance without conviction; delegation per track carries a conviction chosen by the delegator. | [docs.polkadot.com governance](https://docs.polkadot.com/reference/governance/), [Figment: governance breakdown (secondary)](https://www.figment.io/?p=80569), [Bifrost: OpenGov delegations (secondary)](https://medium.com/bifrost-finance/polkadot-opengov-2-0-part-ii-a-deep-dive-into-delegations-fellowship-and-intervention-concepts-7ce7fb7058d2) |

## What Maya2C has (checked in repo, 2026-11-12)

- Direct voting only; weight = locked stake + decayed work credit at cast time (`crates/node/src/state/governance_exec.rs:142-150`, `:485-528`); one ballot per `(proposal, voter)` by key presence (`crates/node/src/governance/mod.rs:56-61`); the lock must outlive execution; quorum denominator is everyone's locked stake plus work credit (`:575-590`, `crates/governance/src/tally.rs:15-24`).
- Compiled floors and ceilings (`crates/governance/src/limits.rs:33-95`, invariant 12).
- No proof of personhood in consensus (`features.toml` personhood RESEARCH), so votes are stake-weighted.

## How the Maya2C version differs

- **Cast-time accumulation, not an epoch snapshot**: the delegate's running sum (`g:dpw:`) is added to its ballot; Cardano tallies against a stake snapshot.
- **Exclusive delegation with a derived lead**: a delegator cannot vote or unlock while delegated, and undelegation completes only after `MAX_VOTING + MAX_TIMELOCK + EXECUTION_GRACE` blocks, so delegated stake outlives every proposal it could have decided, the same guarantee direct voters already carry.
- **Conviction as a shift, on locked stake only**: `locked << c`, never on work credit, with the lock extended by `CONVICTION_PERIOD << c`; the quorum denominator is unmultiplied, as Polkadot's support measure is.
- **One hop**, as DReps; no liquid democracy.
- **Work credit (miners) votes but is never delegated**, which no surveyed chain has an equivalent of.

**Verdict: similar exists (DReps, OpenGov conviction). Differentiate by the derived undelegation lead and the stake-only conviction shift. No "first" claim.**
