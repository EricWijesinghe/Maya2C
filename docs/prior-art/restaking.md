# Prior art: Restaking and shared-security export

**Search status:** searched 2026-11-12 (WebSearch, 4 queries: EigenLayer slashing / Babylon / Symbiotic / Cosmos ICS deprecation). Vendor and secondary sources marked; the Symbiotic docs and the Cosmos forum are primary.

Feature: parity row P-26, [ADR-132](../adr/ADR-132-shared-security-export-and-restaking.md) (OPTIONS FOR ERIC).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| EigenLayer | Ethereum restaking; operators opt in to services (AVSs) whose slashing conditions apply to the restaked ETH. Production slashing activated 17 April (sources give both 2025 and 2026; unresolved). Concentration and slashing cascades are the named risks; one weak source claims about 1,500 operators hold most restaked capital. | [The Defiant](https://thedefiant.io/news/blockchains/eigenlayer-activates-mainnet-slashing-on-april-17-implementing-elips-002-004-ecb683e0), [blockeden (vendor)](https://blockeden.xyz/blog/2026/04/18/eigenlayer-avs-slashing-activation-15b-restaking-reality-check/), [Blockworks 2023](https://blockworks.com/news/eigenlayer-at-risk-of-centralization), [Blockdaemon (vendor)](https://www.blockdaemon.com/blog/eigenlayer-mainnet-slashing-now-live) |
| Symbiotic | Vaults with an immutable slasher choice: none, instant, or VetoSlasher (resolvers may veto in a window; an unvetoed request counts as approved). One network's penalty reduces collateral left for the others, each capped by the guarantee captured at request time. | [Symbiotic docs: slashing](https://docs.symbiotic.fi/modules/vault/slashing), [resolvers](https://docs.symbiotic.fi/core-modules/resolvers), [vault handbook](https://docs.symbiotic.fi/handbooks/Handbook%20for%20Vaults) |
| Babylon | Bitcoin staking: a UTXO with a timelock path and a burn path spendable by an extractable one-time signature; signing two blocks at one height leaks the key. A covenant committee (M-of-N multisig) enforces unbonding and slashing because Bitcoin Script cannot. | [Babylon docs: BTC staking](https://docs.babylonchain.io/docs/introduction/btc-staking), [Babylon Labs blog](https://babylonlabs.io/blog/babylon-s-bitcoin-staking-contract) |
| Cosmos Interchain Security | v1 replicated the Hub's full set to consumer chains; v2 "partial set security" lets a top-N or opt-in subset secure a consumer (used by Stride and Neutron). The Hub is removing ICS: a two-phase proposal (stop new consumers, delete the module) calls it heavy technical debt; Gaia v28, which removes ICS, was on the public testnet in August 2026; the mainnet vote result was not found. | [Cosmos forum: remove ICS](https://forum.cosmos.network/t/remove-ics-from-the-cosmos-hub-software-upgrades-to-follow/16682), [why ICS deprecation](https://forum.cosmos.network/t/why-ics-deprecation/16333), [Hub weekly update 2026-08-21](https://forum.cosmos.network/t/hub-weekly-update-13-august-21-2026/17255), [ICS docs: partial set security](https://cosmos.github.io/interchain-security/features/partial-set-security) |

Not found: a post-quantum equivalent of extractable one-time signatures for ML-DSA or SLH-DSA (not searched beyond the Babylon results).

## What Maya2C has (checked in repo, 2026-11-12)

- One bond securing DAG-BFT only; `Params` at `crates/node/src/state/staking.rs:185-194`; evidence-based equivocation slashing verified by the node (`state/staking_exec.rs:256-268`).
- No export, no opt-in duties, no external slasher. Invariant 11 (no oracle by default) and 12 (governance cannot make governance unsafe) stand in the way of any external slashing source.
- ADR-096 derives the exit window from vault delay + unlock + unbonding; a second unbonding clock would change it.

## How a Maya2C version would differ

- Any fault must be two Maya2C-verifiable signed messages or a missed-attestation count evaluated on chain (ADR-028's model); no key-extraction trick exists for lattice signatures in the tree, and no outside verdict is accepted (invariant 11).
- Exposure capped in governed basis points per duty and per epoch, inside governance range checks (invariant 12), so a duty cannot empty a committee seat.
- Duties, if any, are Maya2C's own (ADR-123 sampling, ADR-133 storage, ADR-134 checkpoints), paid from escrow, never issuance (ADR-044).

**What may be said publicly:** nothing; no option is chosen. **Forbidden:** "first PQ restaking", "only", "restaking without risk".

**Verdict:** similar exists (EigenLayer, Symbiotic, Babylon, ICS v2); the originator of the shared-security pattern most like the brief is removing it. Recommendation in ADR-132 is no export at launch; Eric decides.
