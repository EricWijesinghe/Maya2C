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

## Amendment 2026-11-18: one byte form per VRF key and proof

Pre-approved by Eric on 2026-11-17 into re-freeze bundle 3 (PR #596).

The beacon's VRF now accepts a public key and a proof's gamma only in their
canonical point encoding (spec rule CRY-5). Point decompression alone accepts
y ≥ p, so one point had several 32-byte forms. Verification hashes the
re-compressed gamma, so before this a proof could be re-encoded and still
verify: two proof byte strings for one beacon value. Verification also salts
hash-to-curve with the key's raw bytes, so a re-encoded key would have been a
second key with different outputs. Keys and proofs this code generates are
always canonical, so honest output is unchanged; only re-encoded inputs are
refused. The response scalar was already required to be reduced.

Invariant 10 (suite octet `0x03`) and the RFC 9381 vectors are unchanged.
Before genesis, no stored registry key or proof exists to check. Tests:
`a_public_key_with_a_non_canonical_encoding_is_refused` and
`a_proof_with_a_non_canonical_gamma_is_refused` in
`crates/vrf/tests/encoding_tests.rs`.
