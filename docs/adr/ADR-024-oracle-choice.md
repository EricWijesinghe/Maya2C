# ADR-024: Oracles — the native module, optional, with a named authority set until staking exists

**Status:** Accepted
**Date:** 2026-09-26

## Context

Master Prompt 17 §5: a native oracle module for mainnet core (a staked
signer set, median aggregation, staleness checks), or a documented plan for
external oracle networks. Choose in an ADR.

## What exists

`crates/node/src/oracle` has:

- a multi-signed price feed and randomness beacon;
- median aggregation over a quorum: moving the median needs a majority of
  liars, not one (`feed.rs`);
- freshness measured in block height, never timestamps (invariant 9);
- optionality: absent by default, no `o:` records, the state root unchanged
  (invariant 11).

## Decision

**The native module**, because it exists, is tested and is bounded by
invariants. An external oracle network would need a bridge whose security
this chain cannot audit.

The authority set is **named in genesis, not staked**. Staking is not built
(`spec/06-staking.md`), so "staked oracle set" is not available. A named set
is the honest description of the trust being introduced (`oracle/mod.rs`
calls it "the chain's first trusted party"). When staking exists, a MIP binds
oracle membership to stake and makes a provably wrong signed price slashable.

## Consequences

- A network that wants prices must write its authorities into genesis, and
  that is a governance decision somebody signs.
- No GraphQL indexer or subgraph template is built. Indexer APIs are listed
  as a gap in `reports/17-integrations.md`.
