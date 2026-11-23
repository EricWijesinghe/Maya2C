# Prior art: Appchains secured by a base chain's validators

**Search status:** searched 2026-11-12 (WebSearch, 3 queries: Cosmos ICS v2 / Avalanche ACP-77 / Polkadot Agile Coretime). Avalanche docs, the Polkadot RFC and the Cosmos forum are primary.

Feature: parity rows P-37 and P-14, [ADR-134](../adr/ADR-134-appchains-secured-by-maya2c-validators.md) (OPTIONS FOR ERIC).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Avalanche L1s (ACP-77, Etna) | An L1 runs its own validators; instead of staking 2,000 AVAX each, L1 validators keep a P-Chain balance and pay a dynamic continuous fee (initially about 1.3 AVAX/month) computed per P-Chain block from the number of active L1 validators; unused balance is returned on exit. L1 validators sync only the P-Chain for validator-set changes and cross-L1 messaging; the primary network executes nothing of the L1. | [ACP-77 (docs.avax.network)](https://docs.avax.network/docs/acps/77-reinventing-subnets), [How L1 validator fees work](https://docs.avax.network/blog/l1-validator-fee), [Etna changes (academy)](https://academy.avax.network/guide/etna-changes) |
| Polkadot (Agile Coretime, RFC-1) | Relay-chain validators execute and validate parachain blocks (replicated security). Slot auctions replaced by coretime sales: bulk (28-day NFT regions, splittable, interlaceable) and instantaneous. A core is an active validator set assigned to parablock validation. | [RFC-1 Agile Coretime](https://paritytech.github.io/RFCs/approved/0001-agile-coretime.html), [Polkadot wiki](https://wiki.polkadot.network/docs/learn-agile-coretime), [Parity: pricing model](https://parity.io/blog/agile-coretime-pricing-model) |
| Cosmos Interchain Security | v1 replicated the Hub's full validator set to consumer chains; v2 partial set security (top-N or opt-in) used by Stride and Neutron; the Hub is removing ICS as technical debt (two-phase proposal; Gaia v28 on public testnet 2026-08; mainnet vote result not found). Validators described the product as "more of a burden than a benefit". | [ICS docs: partial set security](https://cosmos.github.io/interchain-security/features/partial-set-security), [Cosmos forum: remove ICS](https://forum.cosmos.network/t/remove-ics-from-the-cosmos-hub-software-upgrades-to-follow/16682), [Hub weekly update 2026-08-21](https://forum.cosmos.network/t/hub-weekly-update-13-august-21-2026/17255) |

Rollup frameworks (OP Stack, Arbitrum Orbit, ZK Stack) are the P-14 row and were not re-searched here.

## What Maya2C has (checked in repo, 2026-11-12)

- No appchain, no registration, no framework. `crates/l2-flash` is a payment-channel network (`crates/l2-flash/src/lib.rs:1-15`), not a rollup; the P-14 row's "l2-flash REAL/verified" does not make rollups shipped.
- `StateLayer` proofs against finalized headers (`crates/node/src/state/proof.rs:311`) are the exportable fact; ADR-122 (Proposed) puts an Ethereum light client in state and delivers inbound messages only.
- `crates/light-client` verifies PoW headers by cumulative work (`crates/light-client/src/lib.rs:10-14`); the node's production consensus is DAG-BFT, so an appchain verifying Maya2C finality needs ADR-089's finality light client (claimed) or ADR-125's STARK.
- Suite registry and sunset schedule (invariants 29, 30; ADR-082).

## How a Maya2C version would differ

- Avalanche's shape (own validators, continuous fee, anchored checkpoints, no base-chain execution) with ML-DSA/SLH-DSA validator keys under the registered suites, so an appchain inherits the suite sunset schedule; a hybrid signature is 11,165 bytes (`hal/radio-transport/src/lib.rs:5-8`), so checkpoints per block are capped by bytes (invariant 33).
- Checkpoints verified by the node, no reporter (invariant 11); equivocations recorded, not slashed, unless ADR-132's duties are adopted.
- Messaging reuses ADR-122's ordered-nonce rail when the appchain's proof format is Maya2C's own.

**What may be said publicly:** nothing; no option is chosen. **Forbidden:** "first PQ appchain platform", "only", "shared security" without saying whose stake.

**Verdict:** similar exists (Avalanche L1s are the closest shape; Polkadot is the execution shape ADR-134 refuses). No appchain exists or is asked for; Eric decides scope under ADR-016.
