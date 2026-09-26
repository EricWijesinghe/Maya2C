# ADR-020: A spec with an independent reference, and upgrades that halt instead of fork

**Status:** Accepted
**Date:** 2026-09-26

## Context

Master Prompt 15 asks for a written spec, conformance vectors generated from a
reference and run against the node and a second-language verifier, and an
upgrade mechanism under which a node that has not upgraded stops cleanly
rather than forking.

## Decisions

1. **The reference shares no code with the node.** `crates/spec-ref` depends
   on `blake3` and `serde_json` only. A reference that called `ledger-math` or
   `state::merkle` would reproduce their bugs and certify them. It is written
   for reading, not speed.

2. **Signatures are an input to the reference, not recomputed.** ML-DSA and
   SLH-DSA are specified by FIPS 204/205 and checked against NIST ACVP vectors
   (`crates/crypto-pq/tests/acvp_tests.rs`). Re-implementing them in the
   reference would add a third implementation to keep correct without adding
   a check the KATs do not already make. The one node-produced input,
   `spec/tests/keys.json`, is cross-checked: the reference and the TypeScript
   verifier recompute every address from its key.

3. **Every u64 in a vector is a decimal string.** The first TypeScript run
   failed six vectors because `JSON.parse` rounds integers above 2^53; the
   values at `u64::MAX` are exactly the overflow cases the vectors exist for.
   A vector that means different things in different languages is not
   language-neutral.

4. **Rules carry IDs and polarity.** A rule is covered when it has an
   accepting and a rejecting vector, or is marked *positive only* because it
   defines a value, or cites existing external vectors by path (checked to
   exist). `cargo xtask spec-coverage` lists gaps and does not fail on them by
   default; `--strict` does.

5. **Upgrades are scheduled in genesis and halt at their height.** A network
   declares `(version, activation_height)` pairs; a binary knows its
   `SUPPORTED_PROTOCOL_VERSION`. `Chain::insert_block` refuses a block at or
   past the first unsupported version's height with
   `UpgradeRequired` — "upgrade required before height H" — before applying
   any rule. The schedule is hashed into nothing, so existing genesis ids and
   roots are unchanged.

## Consequences and what is not done

- The first run of the suite found a wrong statement in the spec draft (the
  v5 txid does include both signatures). That is the suite working.
- Headers carry **no protocol version field**; the version at a height is a
  function of the schedule. Adding a field changes the 144-byte header and the
  GPU miner's in-place nonce layout, and is left for the first upgrade that
  needs it.
- Governance cannot yet change the schedule: it lives in genesis. Moving it
  into governed state is a follow-up MIP.
- The hash migration is a rehearsed procedure, not a feature (RESEARCH).
- Chain-level rules (state-root check, retarget, PoW, fork choice, prune
  horizon) have no language-neutral vectors yet; they are pinned by node tests.
