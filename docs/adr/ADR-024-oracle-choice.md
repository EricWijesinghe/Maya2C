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

The registry also refuses such a key when it is installed, not only when a
proof under it is verified: `OracleRegistry::new`, which genesis
(`oracle.authorities`), a signed rotation and the stored-record decoder all go
through, calls `VrfPublicKey::point()` for every authority. A key that is not a
point, not its canonical encoding, or of small order is refused as
`InvalidOracleRegistry`. Before this, such a key could enter committed state
and that authority's beacon slots would always take the deterministic
fallback. Every node refused it the same way, so this was never a fork; the
refusal now comes earlier. A stored registry holding such a key would no longer
decode, which matters only for state written before this change, and no
value-bearing chain has an oracle registry yet.

Secret wiping in the VRF is best-effort: the seed digest, the clamped scalar
bytes, the nonce prefix, the nonce hash and the `Scalar`s `x` and `k` in
`prove` are zeroized. Copies the compiler makes and the SHA-512 hasher's
internal block buffer are not reached.

Invariant 10 (suite octet `0x03`) and the RFC 9381 vectors are unchanged.
Tests: `a_public_key_with_a_non_canonical_encoding_is_refused`,
`a_proof_with_a_non_canonical_gamma_is_refused` and
`a_gamma_of_the_identity_with_the_sign_bit_set_is_refused` in
`crates/vrf/tests/encoding_tests.rs`;
`a_registry_refuses_a_vrf_key_no_proof_can_verify_under` and
`a_rotation_to_a_vrf_key_no_proof_can_verify_under_is_refused` in
`crates/node/tests/oracle_tests.rs`.
