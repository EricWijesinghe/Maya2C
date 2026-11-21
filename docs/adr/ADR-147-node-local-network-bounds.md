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

1. **Orphan pool is bounded in blocks and in bytes, with a per-parent share.**
   The 512 limit counts buffered blocks, not parent ids; the pool also holds at
   most 64 MiB of block wire bytes; and one missing parent may hold at most 8.
   A block whose body does not match its header's `tx_root`, or whose wire form
   is over `MAX_BLOCK_BYTES`, is refused (and its sender blamed) before it is
   buffered, since both checks are stateless; `insert_block` runs the same two
   in the same order. After the `tx_root` check the header id commits to the
   whole body, so a block already buffered under its parent, by id, is not
   added again. A block refused for want of room still names its missing
   parent, so an ancestor walk from its relay still starts. When the pool is
   full by count or bytes, a block that a buffered orphan waits on displaces
   the most recently buffered orphan nothing waits on (never one of its own
   children): an ancestor walk fetches newest-first, so without this the top
   of a long gap would fill the pool and refuse the blocks just above the
   tip. The
   per-parent share assumes honest DAG-BFT blocks form a linear `prev_hash`
   chain; raise `MAX_ORPHANS_PER_PARENT` if a legitimate fork can exceed eight
   siblings. Tests (`import::tests`): `a_ninth_orphan_under_one_parent_is_refused_by_import`,
   `the_pool_limit_counts_blocks_not_parents`,
   `the_pool_refuses_at_its_byte_budget_before_its_block_count`,
   `an_oversized_orphan_is_refused_and_blamed_before_buffering`,
   `a_refused_orphan_still_names_its_missing_parent`,
   `a_repeated_orphan_does_not_use_up_the_parent_share`,
   `taking_children_releases_their_share_of_both_totals`,
   `a_copy_with_a_swapped_body_cannot_displace_the_genuine_orphan`,
   `a_walk_into_a_nearly_full_pool_still_lands_a_run_of_blocks`,
   `an_ancestor_that_cannot_be_made_to_fit_evicts_nothing`. Victims are
   counted before any is dropped, so a block that cannot be made to fit
   evicts nothing.
   Both bounds use the compile-time `MAX_BLOCK_BYTES` (the pre-buffer size
   check, and the assertion `MAX_ORPHAN_BYTES >= MAX_BLOCK_BYTES`), as
   `insert_block` does today. If the block byte cap becomes a scheduled
   parameter (DECISIONS.md 2026-11-15), both must follow the scheduled
   maximum, or this path would refuse honest maximum-size blocks. The
   parameter schedule open in this re-freeze (#575) leaves `check_size` on the
   compile-time cap (checked 2026-11-18), so the two agree today.
2. **Block-sync rate table admits a newcomer when full.** After expired windows
   are dropped, the window that started longest ago is evicted. Test:
   `network::sync::tests::a_newcomer_is_admitted_when_the_table_is_full`.
3. **RPC rate table holds its ceiling.** When the sweep leaves the table full,
   the least recently touched buckets are evicted until `EVICT_BATCH`
   (`MAX_TRACKED / 16`) places are free, so the table never exceeds
   `MAX_TRACKED` and the next newcomers do not each pay for a full pass under
   the lock. Tests: `rpc::limit::tests::a_full_table_of_drained_buckets_still_admits_a_new_address`,
   `eviction_at_the_ceiling_leaves_headroom_and_keeps_the_newest`.
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
6. **The double-sign guard needs the current epoch's safety log itself.** Only
   `epoch-N/safety.log` counts, where `N` is the epoch the driver will run (the
   committed staking epoch, else the genesis setup's, as `BftDriver::open`
   chooses it). `SafetyStore::open` creates that file with the directory, so an
   honest validator always has it. Another epoch's log does not count: the
   previous epoch's directory is kept after a new one starts, so it survives the
   loss of the current one, and it says nothing about the current epoch's rounds
   (amended 2026-11-21 after the independent AI review's CON-04). A validator
   whose current log was removed refuses to start without
   `--i-know-this-validator-never-signed`; so does one stopped at an epoch
   boundary before its driver opened the new epoch, which has signed nothing in
   it and may pass the flag. Test:
   `safety_guard::tests::only_the_current_epochs_safety_log_counts`.
7. **The node key file is created owner-only and never overwritten.** Mode 0600
   is set at `open` (Unix), `create_new` refuses an existing file, a failed write
   removes the partial file, the file is synced (`sync_all`) before the node
   goes on, on Unix its directory entry is synced too, and the seed copies are zeroized both when the file is created and when
   it is read at start. Tests:
   `a_new_key_file_is_never_overwritten`, `a_new_key_file_is_owner_only_from_creation`
   (Unix), `a_failed_seed_write_leaves_no_partial_key_file`.
8. **An attestation collector is bound to its epoch.** The driver's collector
   (and the closing epoch's, which keeps it) refuses an attestation of another
   epoch before verifying or counting it. The driver already routes attestations
   by epoch; the binding keeps the rule in the collector, because the same keys at
   the same indices verify in two epochs (review CON-23, 2026-11-21). An unbound
   `Collector::default()` behaves as before. Test:
   `a_bound_collector_refuses_a_valid_attestation_of_another_epoch`.

## Consequences

- Eviction at the ceiling (items 2 and 3) means an entry at its quota whose
  record is oldest regains a full quota once roughly a table's worth of newer
  ids have cycled through. The previous behaviour was worse in each case: the
  sync table refused every newcomer, and the RPC table grew past its ceiling.
- Orphans still never expire (the TTL sweep is left open). Blocks with
  fabricated parents cost nothing to make and never reach `insert_block`, so
  they are never scored. A full pool, or a parent whose share of 8 is held by
  such blocks, makes the node refuse honest out-of-order blocks until backfill
  or certificate replay catches it up. The per-parent share makes that refusal
  targetable at one upcoming parent, not only pool-wide; the refused block
  still starts an ancestor walk, and consensus is unaffected. While such
  blocks hold the whole pool, a walk's blocks are refused too (nothing
  buffered waits on them), so each walk lands only its deepest block after
  downloading the gap: catch-up over gossip costs about k^2/2 block fetches
  for a gap of k. Backfill and certificate replay are unaffected. The TTL
  sweep is the fix and stays open.
- Displacement when full lets a sender that planted a child of its own block
  while the pool had room later displace one newest leaf per block it sends.
  It cannot gain more places than it already held, and the leaf displaced is
  the one least near the tip.
- The orphan pool's displacement and the RPC eviction scan their tables
  (at most 512 orphans; 65,536 buckets, once per `EVICT_BATCH` newcomers).
- The market HTTP cap of 256 is global, with no per-source share. One client
  that keeps reopening idle sockets (each closed after the 10 s head timeout)
  can hold every permit and lock other clients out of the market endpoint.
  This is node-local and does not touch consensus or the JSON-RPC server; the
  previous behaviour let idle sockets hold tasks and file descriptors without
  limit for the whole process. A per-IP share is left open.
- Windows has no mode bits; owner-only ACLs for the key file stay with ADR-049.
- Invariant review (`docs/invariants.md`, 1-34): none is touched. No item
  writes state, reads a clock in a validity rule, or changes what a block,
  transaction or attestation must satisfy; the orphan, rate and connection
  tables are node-local memory, iterated only to pick an eviction victim.
- `pub fn restrict` is removed from the `custom_l1_node` library; no caller
  remains in the repository.
