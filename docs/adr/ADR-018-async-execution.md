# ADR-018: Synchronous execution for mainnet v1; asynchronous (D = 2) is the next step

**Status:** Accepted
**Date:** 2026-09-27

## Context

Master Prompt 12 §2 asks whether consensus should order blocks without
waiting for execution, committing block N's state root in block N + D, and
to evaluate the trade-off honestly.

## Options

**Synchronous (today):** every header carries the post-state root of its own
block; a validator executes before voting. Light clients get a state root
per block with the block; finality means "ordered and executed". Cost:
execution time is on the consensus critical path.

**Asynchronous, root deferred by D:** ordering proceeds while execution runs
behind; header N carries the root of block N − D. Gains: consensus latency
no longer includes execution; ordering and execution pipeline. Costs:
(1) a light client proving a balance "as of block N" must wait for header
N + D — account proofs lag finality by D blocks; (2) an invalid transaction
cannot be refused by consensus, only skipped by execution, so blocks must
charge for included-but-failed transactions or they become a free DoS
channel; (3) a determinism bug surfaces D blocks late, after further blocks
are ordered on top.

## Decision

- **Mainnet v1 is synchronous.** DAG-BFT is not yet in the node (ADR-015),
  and adding a second axis of change to the consensus path at the same time
  multiplies the audit surface. Measured execution costs are small next to
  a 2 s block: the DAG-BFT commit rule costs microseconds per vertex
  (`reports/04-consensus.md`), and a 10,000-transaction block of
  pre-verified synthetic work executes in the tens of milliseconds
  (`reports/12-performance.md`). Signature verification dominates, and it
  happens at ingest, off the critical path, regardless of this choice.
- **When measured execution exceeds ~25% of the block interval**, move to
  asynchronous execution with **D = 2**: two blocks of pipelining covers the
  execution of one block while the next is ordered, and bounds the
  light-client proof lag to ~4 s at 2 s blocks. Included-but-failed
  transactions are charged base fee.

## Consequences

Light clients keep same-block proofs for v1. The pipeline stages Master
Prompt 12 §2 lists (ingest → verify → mempool → order → execute → commit →
persist → index) exist as bounded channels only where they already exist in
the node (block import, gossip); a full staged pipeline is future work.
