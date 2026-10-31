# ADR-072: a block has a byte cap, and the transactions past it wait for the next block

- Status: Proposed (2026-10-31). The value is provisional, pending measurement on the launch host.
- Scope: consensus (block validity and selection), pre-genesis. Takes effect at the re-genesis.
- Spec: `spec/05-consensus.md` CON-12, vectors in `spec/tests/consensus.json`.

## Context

The fee clamp (ADR-071, PR #274) bounds how fast the base fee can rise, so a block's
size no longer raises its price. Before that, a large block was expensive to fill. After it,
size is only a cost for the attacker who pays the fee, and the chain has no other bound on it.

The security review of #274 found three places the size is unbounded:

- `crates/node/src/consensus/bft/builder.rs` passes the whole committed sub-DAG to
  `select_applicable`. Its third argument is the difficulty target, not a size cap.
- `crates/node/src/state/fee_exec.rs` sums the bytes it charges for and nothing else.
- `crates/node/src/consensus/bft/store.rs` caps each vertex at 4 MiB (`MAX_BATCH_BYTES`), and
  the committee is up to 100. A round's causal history after `k` stalled rounds lands in one
  block, so one block can carry `k × 100 × 4 MiB`.

The result is a volume attack on apply time, state growth and propagation.

## Decision

1. `maya_fee_market::limits::MAX_BLOCK_BYTES` is a compiled constant beside the other fee bounds,
   not a configuration key (the same arrangement as ADR-044's limits). Its value is 5 MiB.
2. A block's wire size is its `to_bytes()` length: the 145-byte header, an 8-byte count, and
   per transaction an 8-byte prefix plus the encoding (`block_wire_len`).
3. `StateDB::select_applicable_within` admits **first-fit**, in order. A transaction that would
   take the block over the cap is passed over and the ones after it are still considered; the
   admitted set is a function of the order and the cap alone, so every node makes the same cut.
   A transaction that does not execute costs no room. A transaction too big for the room left is
   not staged at all: whether or not it would execute it is excluded, and staging it and rolling
   back would leave the same state. (A first-fit that stopped at the first overflow was rejected:
   one maximal transaction, or one non-executing one, could hold back every transaction behind it.)
   Its oracle is `core::block::first_fit` and `spec-ref` `consensus::first_fit`.
4. A block over the cap is refused on import (`Chain::admission_checks`, before anything is
   stored, so the refusal costs no proof-of-work check) and on every apply path (`stage_block`,
   which all apply entry points share). An over-cap block is `NodeError::BlockTooLarge` and
   counts as an invalid block for peer scoring.
5. Transactions the cut leaves out are not in `included`, so the driver keeps them in the
   mempool. Ones the node itself queued are returned through `dropped` and proposed again.
   The nonce rule stops a transaction that is proposed again from executing twice.

## Value and derivation (provisional)

The value must sit between two bounds and is checked against the measured data.

- **Lower bound.** A maximal transaction (`MAX_TX_BYTES` = 4 MiB) plus the block's framing
  must fit alone, or no block could ever include it. This is a compile-time assert in `core/block.rs`.
  The cap is at least 4 MiB + 161 bytes.
- **Upper bound.** A block at the cap should still travel in one 8 MiB gossip frame
  (`MAX_GOSSIP_MESSAGE_BYTES`, `network/behaviour.rs`), with room for the envelope.
  Block sync is looser: one response carries up to 24 MiB of payload and 32 MiB on the wire
  (`network/sync.rs`).
- **Measured data.** `reports/12-baseline.md`: apply of one hybrid transfer, one thread,
  optimized, includes verification: 1.062 ms, and a hybrid transaction is 11,165 bytes
  (`docs/blockgraph.md`). That is about 100 ms per MiB, and about 37 ms per MiB with
  four-thread verification (0.324 ms verify + 0.071 ms state work per transaction). These are
  derived from the 2026-09-26 record, not re-measured for this cap.
- **Block cadence.** `reports/bench/2026-10-25-tps-series.md`: a 400-transfer block took about
  2.6 s on a shared machine. 400 transfers is about 4.3 MiB, so the cap admits that block.

5 MiB admits one maximal transaction with about 1 MiB to spare, admits the measured
400-transfer block, and keeps apply near 0.2 s (parallel verification) to 0.5 s (serial) per block,
against a measured block interval of about 2.6 s. **These apply figures are derived, not measured.**
A measurement of apply time per MiB on the launch host is MISSING. It is the condition for
moving this from Proposed to Accepted.

## Consequences

- The measured 400-transfer run (153 transfers/s, `reports/bench/2026-10-25-tps-series.md`) is
  inside the cap. Throughput under any mix larger than 5 MiB per block is capped.
- **Target range follows the cap.** `MAX_TARGET_BLOCK_BYTES` was 16 MiB. A target at or above the cap
  can never be exceeded, so the base fee would never rise and spam would stay at the floor. It is now
  `MAX_BLOCK_BYTES / 2` (2.5 MiB, the published testnet's own target), so a full block is 2x target and
  moves the fee by the full 1/D step (ADR-071).
- A block over the cap is refused on every path, so a malicious builder cannot bypass the cap.
- Consensus-visible: a block that was valid before is now invalid above the cap, and the
  selection changes what an over-cap order produces. Re-genesis only.

6. **Verification warm-up is bounded.** `prewarm_verification` is cache-only: it fills the
   verified-signature cache and ignores failures, and the sequential pass verifies again through
   that cache. It now covers only the longest prefix whose encodings fit the cap
   (`core::block::warmup_len`). Anything later is verified when the pass reaches it, so the warm-up
   costs at most one cap of bytes.
7. **Mempool admission refuses a transaction over `MAX_TX_BYTES`** (`network/mempool.rs`). The
   decoder already refused such a frame; a local submission (`Mempool::insert`) was never decoded
   and could be admitted, and could not fit any block.
8. **Decoding is canonical**, so the size the check measures by re-encoding is the size the peer
   sent. `Block::from_bytes` ends with `ByteReader::finish`, which refuses trailing bytes, and the
   counts are fixed-width. Pinned by `a_received_block_re_encodes_to_the_bytes_it_came_from`.

## Sub-DAG decode bound and fair share (CON-13)

9. **The builder decodes at most `DECODE_BUDGET` of batch bytes per block, shared fairly among
   authors** (deficit round-robin in ascending committee index, CON-13).
   - Before this change, the decode cut ran in commit order `(round, author)`. The lowest index
     took the whole budget first, so a validator that filled its vertices with junk or
     non-executing payloads starved honest proposers. That was the 2026-11-01 security review's
     HIGH-1. An author now spends only its own share, and unused shares go to the others in the
     same walk.
   - `decode_plan` (`core/block.rs`) is the single rule; `builder::decode_fair_payloads` and the
     spec-ref oracle `consensus::decode_fair` use it.
   - **Charging.** The per-author meter charges raw bytes. What it bounds is decode work, which
     happens before anyone knows whether a payload decodes or executes. The block cap (CON-12) is
     still charged only for executing transactions. The alternative, charging only executing bytes,
     cannot bound decode work: a junk payload would be decoded for free.
   - **Quantum divisor.** The quantum is `DECODE_BUDGET / n`, where `n` is the committee size
     (`BftDriver.committee.len()`), not the number of authors with payloads. With the active count,
     one honest author among `f` active junk authors gets `B / (f + 1)` on its first turn, which is
     below the guarantee when `f` is close to `n / 3`.
   - **Prefix.** An author's admitted set is a prefix of its queue. A payload that does not fit the
     budget left finishes that author only, and nothing behind it is passed over.
   - **Repeats.** A payload identical to an admitted one is skipped with no charge and no decode.
     The dedupe compares raw bytes, so it is linear and does no parsing. A finished author's
     remaining payloads are never touched.
   - **Commit order.** Admitted payloads are emitted in commit order. Execution order is the commit
     order restricted to the admitted set.

   **Derivation of `DECODE_BUDGET` = 3/2 × `MAX_BLOCK_BYTES`.** Let `M` = `MAX_BLOCK_BYTES`,
   `B` = `DECODE_BUDGET`, `n` = committee size, `f` = junk authors with `f < n / 3`, and
   `Q = B / n`.
   - In the first turn, authors are served in ascending index, and each has deficit `Q`, so each
     admits at most `Q`. Before the `k`-th author is served, at most `(k − 1) · Q` has been spent,
     so at least `(n − k + 1) · Q ≥ Q` remains. Every author therefore gets its full `Q` in its first
     turn, whatever the authors before it did.
   - Honest authors' first-turn admission is then at least `(n − f) · Q` when each honest author has
     at least `Q` of demand, and `Q = B / n`, so it is `B · (n − f) / n`.
   - With `f < n / 3`, `(n − f) / n > 2 / 3`, so the honest first-turn total is above `2B / 3`.
   - With `B = 3M / 2`, `2B / 3 = M`: one full block cap of honest executing transactions is
     admitted whatever the junk authors do. With `B = M`, the bound would be `2M / 3`, which is why
     the budget is raised.
   - Assumption: each honest author's transfers are at most `Q` (`B / n` = 1.875 MiB at `n` = 4 and
     0.075 MiB at `n` = 100, `B` = 7.5 MiB). A single honest payload larger than `Q` waits for
     deficit to accumulate, so the first-turn bound does not cover it; it is still admitted in a later
     turn while budget remains. A transaction of 4 MiB (`MAX_TX_BYTES`) would be such a payload.
     This is a shortcut in the derivation, not in the rule: the guarantee is stated for standard-sized
     transactions.
   - Cost: decode work per block is at most `B` = 7.5 MiB, up from 5 MiB. Termination: a waiting
     author's deficit grows by `Q` per turn, so it admits a payload of length `L` within `⌈L / Q⌉`
     turns. For `L` ≤ `MAX_TX_BYTES` (4 MiB) and `n` = 100 that is at most 54 turns.

   **Residual.** An author's own junk delays only that author's executable payloads. With `f`
   faulty authors, honest throughput per walk is at least `(n − f) / n` of the decode budget, under
   the assumption above. The sub-DAG itself is still unbounded by the protocol: its rounds are not
   capped (see `dag-bft/src/commit.rs` `order_history`), so the work per commit is linear in the
   committed bytes outside the walk, namely the streamed dedupe over raw bytes. Bounding the sub-DAG
   needs a protocol change, which is not part of this change.

## Activation

- Invariant 33 has no activation height. It is enforced from genesis because the change rides the
  pre-genesis re-genesis: there is no earlier chain on which a block could already exceed the cap.
  Nothing is gated, so the rule is the same on every node by construction.

## Not done

- The end-of-block fallback in `builder.rs` (an empty block when the kept set fails the
  state-root pass) is unchanged. It is deterministic, and with the cap enforced the kept set
  is never over the cap.
- The per-vertex 4 MiB batch limit and the committee multiplier are unchanged. The cap
  bounds the block, not what the mempool accepts.
