# ADR-150: The DAG-BFT ordering cutoff follows the committed anchor sequence; node-local bounds from the 2026-11-19 audit

**Status:** Accepted, 2026-11-21 (Eric 2026-11-17 pre-approval of Claude's
re-freeze recommendations; Eric asked on 2026-11-21 to resume all work).
**Date:** 2026-11-21
**Related:** [ADR-015](ADR-015-consensus-design.md), [ADR-027](ADR-027-dag-bft-in-the-node.md),
[ADR-038](ADR-038-attested-checkpoints.md), [ADR-052](ADR-052-follower-advance-by-certificates.md),
[ADR-060](ADR-060-trusted-block-bootstrap.md) (amended),
[spec/05-consensus.md](../../spec/05-consensus.md).

An independent AI review of the tree (2026-11-19, not an external audit)
proposed fixes for the DAG-BFT engine, the node's BFT driver, the safety log and
the pruned bootstrap. Each finding below was checked against the code before
anything changed, and each change is pinned by a test that fails without it.

Item 1 changes which certificates a committed sub-DAG holds, so it changes
derived block contents in one case (below). Items 2 to 7 change what one node
keeps, refuses, writes or spends; two honest nodes with and without them accept
the same blocks and compute the same state.

## Decision

1. **The ordering cutoff is a function of the committed anchor sequence (CON-01).**
   `Committer::order_history` emits only certificates of round at least
   `previous committed anchor round − GC_DEPTH`. Before, the cutoff was whatever
   the local DAG still held, and the engine garbage-collects after every step
   that commits. A node that committed anchors `K − 2` and `K` in separate steps
   had already dropped rounds the node that committed both in one step still
   held, so the second node ordered more certificates under anchor `K`: a
   different block at the same height from two honest nodes (replayed in
   `crates/dag-bft/tests/commit_determinism.rs`: 57 against 61 certificates on
   the unmodified engine). Every honest node agrees on the anchor sequence, so
   the new cutoff is common to all of them, and it equals the horizon a node
   committing one anchor per step already had: blocks such a node derives are
   byte-identical, and only the batched case changes, to match it.
   Alternative not taken: `anchor.round − GC_DEPTH` (the review's first
   suggestion) changes blocks in the common case too.
   Residual: after `Validator::resume_after(round)` (ADR-038) the DAG holds
   nothing below `round`, so the first anchor after a resume may order fewer
   certificates than peers that never resumed. A resumed node imports blocks
   under attestations instead of building them until the cutoff of the anchor
   it would build lies above the resumed round: `may_build_again` lets a
   follower resumed at `resumed_at` build the anchor committed after `P` only
   when the chain's tip is `P`'s block and `P.round − GC_DEPTH > resumed_at`.
   Keying the gate on `P` rather than on the anchor being built matters when
   anchors are skipped: resumed at 100, with anchors 148 and 150 skipped, the
   anchor at 152 has cutoff 96, and a gate on `152 > 100 + GC_DEPTH + 2` alone
   would have let the follower build it from a sub-DAG lacking rounds 96–99
   that its peers order. The bound is strict because at the resumed round
   itself the follower never saw the resumed anchor ordered. With no skipped
   anchor (`P` two rounds below) the gate opens at the same anchor as the
   earlier `round > resumed_at + GC_DEPTH + 2` window. Pinned by
   `follower_gate_tests` in `crates/node/src/consensus/bft/driver.rs`.
2. **The commit rule runs only after a DAG insertion or a resume (CON-22).** It is
   a pure function of the DAG and the committer, so skipping it when neither
   changed returns the same (empty) result; running it per message cost a scan
   of every uncommitted round while commits stalled.
3. **Round window and parent cap before any signature check (BFT-02, BFT-03,
   CON-10).** A proposal or certificate below the collection horizon or above
   the buffer ceiling, or listing more parents than the committee has members,
   is dropped before its digest, its signatures, the equivocation table and any
   fetch. Certificates: `accept` dropped both cases already, after the quorum of
   verifications. Proposals: below the horizon the engine no longer remembers
   its vote for the slot, so voting there could be a second vote.
4. **One fetch-reply budget for every claimed sender (BFT-04).** `from` is what a
   frame claims, so per-member budgets multiplied by every id a requester claims.
   All members together now get twice one member's budget per resend interval.
5. **The safety log records each own message once, and opens in bounded memory
   (CON-03, CON-11).** The driver skips `record_own` for a proposal or vote
   already logged (key: round, addressee, and the vote digest or the proposal
   signature's hash), seeded at open and pruned with the horizon; a re-sent
   pending proposal appended a full vertex and an fsync per resend interval
   while it stayed uncertified. At open the log is streamed and only the newest
   proposal is kept whole; each older one is restored as the vote for its own
   slot, which still blocks a second signature there. A new epoch directory and
   log get a directory fsync (Unix; std cannot flush a directory on Windows).
6. **The anchored bootstrap ends at the trusted block, and a snapshot import is
   capped (CON-02, CON-15).** See the ADR-060 amendment.
7. **Rejoin recency uses the epoch that signed the trusted block (CON-13).**
   Block `kE` ends epoch `k − 1`, so its signing committee is epoch
   `(h − 1) / E`, not `h / E`; the old formula loosened the window by one epoch
   for a boundary-height trusted block. Node-local start-up check
   (`bins/maya2c-node/src/rejoin.rs`).

## Not decided here

Owner decisions (economics or trust model), no code: CON-05 (attestation and
vote equivocation evidence and its slash), CON-06 (per-member weight cap),
CON-07 (active set below `MIN_COMMITTEE`), CON-08 (unconfirmed time-old tip).
Recorded, not changed: CON-09 (header timestamp unbounded above; no state rule
reads it), CON-12 (an error mid-step drops the rest of the step), CON-14
(attested catch-up holds the whole gap), BFT-06 (gossiped transactions are
validated on the swarm task; the proposed offload ignored any transaction
arriving while four checks were in flight, which turns a stall into silent loss
under a burst, so it needs a bounded backlog and a measurement first).

## Consequences

- Committed block contents change only for a node that commits two or more
  anchors in one step, and only toward what a one-anchor-per-step node derives.
  Pre-genesis; the test network's stored blocks are imported, not re-derived.
- No rule ID and no language-neutral vector for item 1: the commit rule has no
  reference model in `crates/spec-ref` yet (spec/05 gap).
- A node bootstrapped with `--trusted-block` stops at the trusted height until
  catch-up runs; the node already warns when `--catch-up-from` is missing.
