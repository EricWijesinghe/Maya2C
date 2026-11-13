# ADR-062: Choose the accounts tree and the staking record layout before genesis

Status: Accepted (2026-10-27, Eric: both, measure first). Staking (b) and accounts (b) are built behind measurement (`cargo xtask bench-tps` before and after, and block-apply time against account count); what ships in the re-genesis is what the numbers justify. Originally Proposed 2026-10-26.

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

## Amendment 2026-11-14 — ADR-062a, incremental sparse accounts root (built)

Eric 2026-11-14 MCQ: "incremental root now". Built in `crates/node/src/state/root_cache.rs`.

**What it is.** A persistent cache of the sparse accounts tree's interior: the digest of every
node whose subtree holds two or more accounts, under the local-only prefix `smt:` (depth byte +
the node's key bits). A subtree with one account is that account's leaf and one with none is the
empty digest, so neither is stored; the one account under an uncached node is one `RocksDB` seek
into `acct:`. A block rehashes only its touched accounts' paths (at most 256 nodes each). No
fixed bucket depth: recipients are free to choose, so a depth-16 bucket cache (the 2026-10-28
sketch) is one an attacker fills a single bucket of.

**Root value unchanged.** The value is `maya-stateless-core`'s `sparse::root` over every account,
as before. `StateDB::state_root()` stays the full rebuild and is the reference; staging
(`preview_root`, every apply path) reads the cache. Pinned by
`incremental_root_equals_full_rebuild_over_random_histories` (12 seeded histories × 80 steps of
commits, reverts and out-of-band writes; after every step the stored cache equals a fresh build
node for node).

**Atomicity.** The cache delta is staged in the same `WriteBatch` as the accounts on all four
commit paths (`apply_block`, `apply_block_checked`, `apply_journaled_with`, `revert_block_with`).
The bare key `smt:` holds the committed root the cache describes and is present only while it is
valid: `put_account`, `raw_put` on `acct:`, and any `write_batch` that writes `acct:` (genesis
seeding, snapshot import, purge) delete it, and so does any commit whose post-state is not
sparse. The next commit after a deletion rebuilds the cache from every account, once.
`Chain::open` runs `verify_root_cache`, a node-by-node comparison with a full rebuild (O(N) reads
and hashes, O(depth) memory) that rebuilds on any difference. Snapshots never carry `smt:`
(`LOCAL_ONLY_PREFIXES`, invariant 25), so a snapshot-restored or pruned node builds its own.

**What it does not do.**

1. **It does not change the genesis root format.** The sparse tree is still the dark
   `FeatureSet::STATELESS` feature (`STATELESS_ACTIVATION_HEIGHT = u64::MAX`); at genesis the
   accounts root is the dense address-ordered tree (`state::merkle`), and no cache is kept. The
   dense tree cannot be made incremental at the same root value: inserting an account shifts the
   position of every account after it. Removing the per-block O(N) accounts cost at genesis
   therefore means activating the sparse tree at or near genesis (the `STATELESS` feature bit,
   which `UpgradeSchedule::features` reads; activating it from a genesis schedule entry was not
   tried here), and that also retires the dense `account_proof` the light client's SPV path
   verifies today. That is a decision for Eric, not taken here.
2. **Record layers stay full rebuilds.** Every `RECORD_LAYERS` prefix, channels, contracts and
   nullifiers are dense `merkle_root`s over key-ordered leaves, rebuilt every block (205-213 ms at
   100k records, test profile, `reports/sessions/2026-11-13-per-block-scan-bounds.md`). The same
   insertion-shift argument applies; making them incremental is a root-format change of its own.

## Consequences (062a)

No spec rule changes and no conformance vector changes: the root is the same function of state.
`spec/03-state.md` records that the value is the full function and an implementation may compute
it incrementally.
