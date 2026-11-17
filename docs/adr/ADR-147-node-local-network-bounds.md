# ADR-147: Node-local bounds on network, RPC and validator-guard inputs

**Status:** Accepted, 2026-11-17 (Eric 2026-11-17 pre-approval, re-freeze bundle 3).
**Date:** 2026-11-17
**Related:** [ADR-049](ADR-049-sealed-validator-keys.md) (key files),
[spec/05-consensus.md](../../spec/05-consensus.md), [spec/07-networking.md](../../spec/07-networking.md),
PR #589.

None of this changes a validity rule, a state root, a gas figure or the wire
format. Two honest nodes with and without these changes accept the same blocks
and compute the same state. Each item changes what one node keeps, refuses or
spends while it ingests input, and each is pinned by a test that fails without it.

## Context

A review of the node's network ingestion, its RPC rate limiting and the
double-sign guard found tables and buffers whose limit counted the wrong thing,
or that refused honest newcomers once full, plus a key file written with default
permissions before being tightened.

## Decision

1. **Orphan pool counts blocks, with a per-parent share.** The 512 limit counts
   buffered blocks, not parent ids, and one missing parent may hold at most 8.
   This assumes honest DAG-BFT blocks form a linear `prev_hash` chain; raise
   `MAX_ORPHANS_PER_PARENT` if a legitimate fork can exceed eight siblings.
   Tests: `import::tests::one_parent_cannot_hold_the_whole_pool`,
   `the_pool_limit_counts_blocks_not_parents`.
2. **Block-sync rate table admits a newcomer when full.** After expired windows
   are dropped, the window that started longest ago is evicted. Test:
   `network::sync::tests::a_newcomer_is_admitted_when_the_table_is_full`.
3. **RPC rate table holds its ceiling.** When the sweep frees nothing, the least
   recently touched bucket is evicted, so the table never exceeds `MAX_TRACKED`.
   Test: `rpc::limit::tests::a_full_table_of_drained_buckets_still_admits_a_new_address`.
4. **Market HTTP caps live connections and bounds the request head.** At most 256
   live connections; a socket past that is closed unanswered. The request head
   must arrive within 10 s. Tests: `a_connection_beyond_the_cap_is_refused_not_served`,
   `an_unfinished_request_head_is_closed_after_the_header_timeout`.
5. **A repeat of a counted attestation is ignored before signature checks.** A
   frame naming the same `(epoch, height, validator, block)` as one already
   counted returns `Ignored` without verification. Only verified attestations
   enter the `seen` table, so nothing unverified is ever counted. Both driver
   call sites act only on `Equivocation`, so no penalty path changes. Test:
   `a_replay_of_a_counted_attestation_is_ignored_before_verification`.
6. **The double-sign guard needs the safety log itself.** An `epoch-*`
   directory counts only if it contains `safety.log`. `SafetyStore::open` creates
   that file with the directory, so an honest validator always has it. A
   validator whose log was removed now refuses to start without
   `--i-know-this-validator-never-signed`. Test:
   `safety_guard::tests::only_an_epoch_directory_counts_as_a_safety_log`.
7. **The node key file is created owner-only and never overwritten.** Mode 0600
   is set at `open` (Unix), `create_new` refuses an existing file, a failed write
   removes the partial file, and the seed copy is zeroized. Tests:
   `a_new_key_file_is_never_overwritten`, `a_new_key_file_is_owner_only_from_creation`
   (Unix), `a_failed_seed_write_leaves_no_partial_key_file`.

## Consequences

- Eviction at the ceiling (items 2 and 3) means an entry at its quota whose
  record is oldest regains a full quota once roughly a table's worth of newer
  ids have cycled through. The previous behaviour was worse in each case: the
  sync table refused every newcomer, and the RPC table grew past its ceiling.
- The RPC eviction is a linear scan under the mutex while the table is full
  (a `shortcut:` note in the code names the LRU upgrade).
- The safety guard still checks every epoch directory, not only the current
  epoch's; scoping it needs the epoch passed in and is left open.
- Windows has no mode bits; owner-only ACLs for the key file stay with ADR-049.
- Invariant review (`docs/invariants.md`, 1-34): none is touched. No item
  writes state, reads a clock in a validity rule, or changes what a block,
  transaction or attestation must satisfy; the orphan, rate and connection
  tables are node-local memory, iterated only to pick an eviction victim.
- `pub fn restrict` is removed from the `custom_l1_node` library; no caller
  remains in the repository.
