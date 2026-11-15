# Prior art: Pull oracle with on-chain report verification

**Search status:** searched 2026-11-12 (WebSearch, 2 queries: "Pyth pull oracle updatePriceFeeds on-chain verification Chainlink Data Streams verifier contract"; "Chainlink Data Streams verifier proxy verify report fee docs"). Chainlink pages were read from search snippets only.

Feature: parity row P-46, [ADR-112](../adr/ADR-112-pull-oracle-on-chain-reports.md).

## Who else does something similar

| Project | What it does | Source |
|---|---|---|
| Pyth | Pull model: fetch a signed update from the off-chain Hermes service, call `updatePriceFeeds()` with it, pay `getUpdateFee()`, read the price, all in one transaction; the on-chain contract verifies Wormhole guardian signatures and a Merkle proof against a Pythnet root. | [Pyth blog: pull, don't push](https://pyth.network/blog/pyth-a-new-model-to-the-price-oracle), [Pyth docs (price feeds)](https://docs.pyth.network/llms-price-feeds.txt), [ChainScore architecture note (tertiary)](https://www.chainscorelabs.com/guides/decentralized-finance-defi/decentralized-oracles-and-price-feeds/pyth-network-price-feeds-architecture) |
| Chainlink Data Streams | Low-latency reports fetched off chain and verified on chain through a verifier proxy (`verify()` / `verifyBulk()`); fee handling differs by billing model (subscription: no per-report fee; token billing: LINK or an ERC-20). | [Onchain report verification](https://docs.chain.link/data-streams/reference/data-streams-api/onchain-verification), [Streams Trade interface](https://docs.chain.link/data-streams/reference/streams-trade-interface), [EVM verification tutorial](https://docs.chain.link/data-streams/tutorials/streams-direct/evm-onchain-report-verification) |

## What Maya2C has (checked in repo, 2026-11-12)

- A push feed where anyone may relay a quorum's signed observations (`submit_feed`, `crates/node/src/state/oracle_exec.rs:266-370`, no sender check); one hybrid signature per authority per feed (`oracle_payload.rs:57-80`); lower median; freshness by height (`oracle/feed.rs:93-99`, invariant 9); optional and absent by default (invariant 11).
- Contract reads: `oracle_read`, `oracle_read_u256`, `oracle_feed_age` (`crates/vm/src/runtime.rs:889-927`).
- No batch signing, no atomic update-and-use, no defined off-chain delivery.

## How the Maya2C version differs

- **Post-quantum signatures** (ML-DSA-65 + SLH-DSA hybrid) on every report; Pyth's guardians and Chainlink's DON sign with ECDSA.
- **Native verification by the node**, not by a contract: no signature host function and no verifier proxy; the report is a transaction payload the apply path checks.
- **Atomic use by wrapping**: `WithReports { reports, inner }` applies reports then the inner kind; a lost race is a no-op (invariant 7), a lie refuses the transaction.
- **One trusted set**: no second attestation layer (Wormhole); the registry's quorum and rotation are the whole trust model (`docs/oracle.md`).
- **Freshness is a height** everywhere; signed observation heights are not used for staleness.

**Verdict: similar exists (Pyth, Data Streams). Differentiate by post-quantum signing, node-native verification and the no-op race rule. No "first" claim.**
