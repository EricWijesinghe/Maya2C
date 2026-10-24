# ADR-058: Catch-up fetches blocks in batches

Status: Accepted (2026-10-25). Not consensus: no state, block or wire format changes.

## Context
On 2026-10-24 the PC peers looked hung for hours. They were catching up through
an SSH link of ~0.7 s per call, one block per request, four in flight: about
5 blocks/s, silent, with the RPC down until done (ADR-057). Raising workers
would hit the per-client rate limit (ADR-043 prefetch notes), so the cost is
round trips, and the fix is fewer of them.

## Decision
- New RPC `get_blocks_by_height_range [from, count]`, `count` 1..=32, answered
  from the same lookup as `get_block_by_height` (canonical index, cold archive).
  It stops at the first height the node lacks; the answer is short, not padded.
- `CheckpointSource::blocks(from, to)` defaults to one-by-one, so any source
  and any peer without the method behaves exactly as before. The RPC source
  falls back on "Method not found".
- `catchup::fetch` asks in chunks of `BLOCKS_PER_REQUEST` (32), 16 chunks per
  window, `PREFETCH_WORKERS` in flight. A chunk with fewer blocks than asked is
  an error, never a gap. Linking and execution are unchanged: every block is
  still hash-linked to the attested checkpoint and re-executed (invariant 24).

## Consequences
- Same four requests in flight carry 32x the blocks; the measured gain on the
  live testnet link is recorded in `reports/` once a peer is rebuilt on this.
- Old peers serve catch-up unchanged; new peers serve old nodes unchanged.
- Not done here: progress output while catching up, and serving RPC during it.
