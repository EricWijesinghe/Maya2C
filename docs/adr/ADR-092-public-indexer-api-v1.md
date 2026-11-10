# ADR-092: Public indexer API v1 at `/api/v1`

**Status:** Accepted, 2026-11-10
**Date:** 2026-11-10
**Related:** [ADR-046](ADR-046-universe-scale-supply.md) (256-bit amounts), [ADR-081](ADR-081-address-text-bech32m.md) (Bech32m addresses), [ADR-083](ADR-083-hub-wallet-and-chat-pages.md); `apps/explorer/src/api/`, `crates/web-security/src/rate_limit.rs`, `apps/explorer/openapi/v1.yaml`

## Context

Wallets and dashboards read the chain through the explorer's `/api/*`, which
was written for the explorer's own pages. It has no stable shapes, no
pagination contract, no error codes, no rate limit, and no spec. The gateway
crate `crates/api-gateway` already owns `/v1/*`, so the public indexer API
cannot take that prefix.

## Decision

1. **Path and status.** The indexer API lives at `/api/v1/*` in
   `apps/explorer`. The unversioned `/api/*` keeps working and every response
   on it carries `Deprecation: true` and a `Link` to the v1 successor.
2. **Read-only GET.** History (blocks, transactions, the address index) comes
   from the store. Live values (balance, nonce, validators, supply, mempool,
   chain info, checkpoint) come from the node's JSON-RPC through a TTL cache
   with single-flight: concurrent misses share one node call. `/supply` has a
   30 s TTL because the node scans every account to produce it.
3. **Pagination.** `limit` is 1-100, default 25. The response is
   `{items, next_cursor}`. The cursor is opaque: a version byte, then height
   and position, base64url. Rows are ordered by `(height, position)`, so a
   walk is stable across inserts above the cursor and sees no duplicate or
   gap. A rollback deletes rows from the store, so a walk never returns
   abandoned-branch rows.
4. **Encoding.** Amounts are decimal strings (256-bit, ADR-046). Hashes are
   lowercase hex. Addresses are Bech32m only (ADR-081): hex input is refused
   with `invalid_address`, a valid address for another network with
   `wrong_network`. Timestamps are unix seconds.
5. **Errors.** RFC 9457 `application/problem+json`, with a stable `code`:
   `invalid_address`, `wrong_network`, `invalid_cursor`, `invalid_param`,
   `not_found`, `rate_limited`, `node_unavailable`, `index_behind`,
   `no_staking`.
6. **Rate limit.** The per-client limiter moves from `crates/api-gateway` to
   `crates/web-security` behind a `rate-limit` feature. `api-gateway`
   re-exports it with no behaviour change. The node binary does not link
   `api-gateway` or `tiers` (checked with `Cargo.toml` manifests), so the move
   does not touch the frozen node. Defaults: 10 requests/s, burst 30. A 429
   carries `Retry-After` and `RateLimit-Limit`, `RateLimit-Remaining`,
   `RateLimit-Reset`. Only `ClientIp::Socket` is the default; the header
   modes are for a deployment behind a proxy that overwrites the header.
7. **Caching.** A block or transaction at or below the finalized height
   (`get_checkpoint`) is `Cache-Control: public, max-age=31536000, immutable`.
   Anything above it is `max-age=5`. Live data is `no-cache` with an `ETag`
   over the body and `If-None-Match` returns 304.
8. **Store.** The index gains `tx_outputs(txid, idx, recipient, amount)` and
   `IndexedOutput`. The index generation is bumped to 3, which empties the
   derived tables once so every block is re-read with outputs. Both stores
   gain keyset pages for blocks, transactions and transactions by address
   (sent, received or both), and `oldest_height`.
9. **Spec.** The OpenAPI document is produced in code by `utoipa`. The golden
   file `apps/explorer/openapi/v1.yaml` is compared to it by a test, and the
   test asserts every route is in the document.

## Consequences

- `/api/v1` is a compatibility promise from this point. Breaking it needs
  `/api/v2`.
- `tx_outputs` is written in the same SQL transaction as the block, so the
  index never shows a transaction without its outputs.
- The Postgres SQL for the new queries is verified only by review in this
  change. The repository has no PostgreSQL test harness, so those queries are
  NOT VERIFIED against a server; the `MemoryStore` tests prove the logic.
- The golden file is JSON, which is valid YAML 1.2. Emitting block-style YAML
  would need a YAML serializer dependency, which is not in the lockfile.
- SDKs are out of scope for this ADR.
