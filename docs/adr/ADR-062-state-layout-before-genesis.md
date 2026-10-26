# ADR-062: Choose the accounts tree and the staking record layout before genesis

Status: Proposed (2026-10-26). Needs Eric's decision. Both are state-root format changes: cheap before the mainnet genesis, a hard fork after it.

## Context
The freeze-readiness optimisation review (2026-10-26) found two costs that grow
with chain size and are locked into the state-root format.

1. **Accounts tree.** `StateDB::root_with_overlay` (`state/db.rs`) re-reads every
   account each block, hashes every leaf and rebuilds the whole dense binary
   tree (`state/merkle.rs`). Cost per block is O(all accounts), paid by the
   producer and every validator. A sparse tree already exists
   (`state/stateless.rs`, `sparse::root`), switched on by
   `STATELESS_ACTIVATION_HEIGHT`, currently `u64::MAX` (`docs/stateless.md`).
2. **Staking record.** `state/staking.rs` stores every validator and delegation
   in one value under one key, decoded and re-encoded whole on every staking
   transaction and each epoch boundary. Someone who mass-delegates dust makes
   every staking transaction costlier. The value also outgrows the old 65,536
   byte decode cap (fixed in the same change as ADR-061).

## Options
- **Accounts:** (a) keep the dense tree, tune the I/O only; (b) activate the
  sparse tree from genesis, which allows incremental update and witness-based
  stateless verification, at the cost of resolving the open questions in
  `docs/stateless.md` (how witnesses travel; the end-of-block passes).
- **Staking:** (a) keep one record; (b) one record per validator and per
  delegation, key-ordered, so a transaction touches only its own records.

## Recommendation
Staking (b) and accounts (b), decided together and shipped in one re-genesis,
each behind its own measurement: `cargo xtask bench-tps` before and after, and a
block-apply time against account count (MISSING today). Do not ship a state-root
change that has not been measured.

## Consequences
Light-client proofs and `spec-ref` depend on the tree shape, so the spec and
conformance vectors change with it. The live testnet cannot follow either; both
need the planned re-genesis.
