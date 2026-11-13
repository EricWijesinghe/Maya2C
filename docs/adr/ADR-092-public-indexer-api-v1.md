# ADR-092: Public indexer API v1 at `/api/v1`

**Status:** Proposed (PR #414; the owner's general approval, Eric 2026-11-14, is recorded in DECISIONS.md; Accepted once the PR merges)
**Date:** 2026-11-10, amended 2026-11-14
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
   `{items, next_cursor}`. The cursor is opaque: a version byte, a byte
   naming the list that issued it (blocks; transactions, shared by
   `/transactions` and an address's transactions since both page the same
   order; one block's transactions), then height and position, in hex. A
   cursor from another list is `invalid_cursor`: a block cursor `(h, 0)` read
   as a transaction key would silently skip height `h`. Rows are ordered by
   `(height, position)`, so a walk is stable across inserts above the cursor
   and sees no duplicate or gap. A rollback deletes rows from the store, so a
   walk never returns abandoned-branch rows.
4. **Encoding.** Amounts are decimal strings (256-bit, ADR-046). Hashes are
   lowercase hex. Addresses are Bech32m only (ADR-081): hex input is refused
   with `invalid_address`, a valid address for another network with
   `wrong_network`. Timestamps are unix seconds.
5. **Errors.** RFC 9457 `application/problem+json`, with a stable `code`:
   `invalid_address`, `wrong_network`, `invalid_cursor`, `invalid_param`,
   `not_found`, `rate_limited`, `node_unavailable`, `index_behind`,
   `no_staking`, `internal` (500; the server could not identify the client
   for its rate limit, which is a serving bug, not the client's).
6. **Rate limit.** The per-client limiter moves from `crates/api-gateway` to
   `crates/web-security` behind a `rate-limit` feature. `api-gateway`
   re-exports it with no behaviour change. The node binary does not link
   `maya-web-security` at all (`cargo tree -p maya2c-node -i maya-web-security`,
   output in the PR), so the move does not touch the frozen node. The
   explorer's listener is served with connect info so the limiter has the
   peer address; `tests/server_tests.rs` checks a 200 and a 429 over the real
   socket. Defaults: 10 requests/s, burst 30. A 429
   carries `Retry-After` and `RateLimit-Limit`, `RateLimit-Remaining`,
   `RateLimit-Reset`. Only `ClientIp::Socket` is the default; the header
   modes are for a deployment behind a proxy that overwrites the header.
7. **Caching.** A block or transaction at or below the finalized height
   (`get_checkpoint`) is `Cache-Control: public, max-age=31536000, immutable`.
   Anything above it is `max-age=5`. Live data is `no-cache` with an `ETag`
   over the body and `If-None-Match` returns 304.
8. **Store.** The index gains `tx_outputs(txid, idx, recipient, amount,
   height, position)` and `IndexedOutput`. The index generation is bumped to
   4 (master used 3 for the `explanation` column meanwhile), which empties the
   derived tables once so every block is re-read with outputs. Both stores
   gain keyset pages for blocks, transactions and transactions by address
   (sent, received or both), `transaction_placed` (a transaction with its
   stored position), and `oldest_height`.
9. **Query cost.** Every public list is an index range read newest first
   and stopped at the limit, whatever the client sends. Indexes:
   `transactions (height, position)`, `transactions (sender, height,
   position)`, `tx_outputs (recipient, height, position)`. No cursor binds a
   `(i64::MAX, i64::MAX)` bound instead of `$n IS NULL OR ...`, so one
   statement serves both cases and its generic plan keeps the index
   condition. An address's history is a UNION of a sent branch and a
   received branch, each limited on its own index; the received branch uses
   `DISTINCT ON (height, position)` so several outputs of one transaction to
   the address count once. `tests/postgres_store_tests.rs` EXPLAINs the
   shipped statements as generic plans over 20,000 rows, from the tip and
   from a deep cursor, and fails on any sequential scan.
10. **Spec.** The OpenAPI document is produced in code by `utoipa`. The golden
   file `apps/explorer/openapi/v1.yaml` is compared to it by a test, and the
   test asserts every route is in the document.

## Consequences

- `/api/v1` is a compatibility promise from this point. Breaking it needs
  `/api/v2`.
- `tx_outputs` is written in the same SQL transaction as the block, so the
  index never shows a transaction without its outputs.
- The Postgres SQL runs against a real server in
  `apps/explorer/tests/postgres_store_tests.rs`: one scenario (lookups, keyset
  walks at several limits, address directions, hostile input, reorg and
  rollback, restart and generation rebuild) is run on both `MemoryStore` and
  `PostgresStore`, so the fake the API tests use and the SQL cannot drift. The
  server comes from `MAYA_EXPLORER_TEST_PG`; CI's nextest job and the
  nightly coverage job provide a `postgres:17-alpine` service. Without the
  variable those tests print SKIPPED locally and fail under `CI`, so a job
  that lost its server cannot pass having checked no SQL. `sqlx` was already
  in the crate, and its 0.9 `query()` accepts only `'static` SQL, so a value
  cannot be spliced into a statement without an explicit `AssertSqlSafe`
  (none in `src/`).
- New direct dependencies of `apps/explorer`: `utoipa` (moved to
  `[workspace.dependencies]`, since `api-gateway` already used it; already a
  `cargo vet` exemption at 5.5.0), `tower_governor` and `sha2` (workspace),
  and the dev-dependencies `tower` and `http-body-util` (workspace).
  `maya-web-security` gains optional `governor`, `tower_governor`,
  `thiserror` and `tokio` behind `rate-limit`. The lockfile gains no new
  package, only these edges.
- **Deploying this resets the explorer index.** Moving from generation 3 to
  4 truncates `blocks`, `transactions` and `tx_outputs`, and the indexer
  re-reads the chain from the node. Against a pruned node, history below the
  prune point cannot be re-read and is gone from the index (the 2026-10-12
  explorer freeze was this case). Deploy it against an archive node, or at
  re-genesis when there is no history to lose.
- The golden file is JSON, which is valid YAML 1.2. Emitting block-style YAML
  would need a YAML serializer dependency, which is not in the lockfile.
- SDKs are out of scope for this ADR.
