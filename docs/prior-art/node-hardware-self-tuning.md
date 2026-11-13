# Prior art: a node that sizes its own thread pools and caches from the hardware it finds

**Search status:** not searched. This file authorises no comparative wording.

Feature: HW-2, [ADR-137](../adr/ADR-137-hardware-intelligence.md).

Sizing caches and pools from detected memory and cores is ordinary practice
in databases and node software, and ADR-137 makes no claim that it is new.
Known examples, from general knowledge and **not checked in a dated search**:
database engines that size buffer pools as a share of memory, RocksDB's own
option helpers for point-lookup and level-style workloads, and blockchain
clients that expose a cache-size flag with a memory-derived default.

## What Maya2C has (checked in repo, 2026-11-12)

- Fixed defaults in `StorageConfig` (`crates/node/src/config.rs:108`).
- A compiled `MIN_PER_THREAD = 32` and `available_parallelism()` in
  `prewarm_verification` (`crates/node/src/state/db.rs:1175-1179`).

## What ADR-137 adds that is specific to this tree

- A written rule and a test that a tuning choice changes speed and never a
  block's fate or a state root.
- A deny list of knobs a profile may not touch.

**Verdict: common practice. No "first" claim, and none would be authorised
by this file.**
