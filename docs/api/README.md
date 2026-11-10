# Maya2C indexer API, v1

Read-only HTTP API for wallets and dashboards. Base path `/api/v1`, served by
the explorer (`apps/explorer`). Decision: [ADR-092](../adr/ADR-092-public-indexer-api-v1.md).
The contract is [`apps/explorer/openapi/v1.yaml`](../../apps/explorer/openapi/v1.yaml).

## Conventions

- **GET only.** Anything else is refused by the router.
- **Amounts** are decimal strings, 256-bit (ADR-046). Never parse them as JSON numbers.
- **Hashes** are lowercase hex (64 characters).
- **Addresses** are Bech32m (ADR-081). Hex is refused on input. On output a
  field is `null` until the node has named its chain.
- **Timestamps** are unix seconds.
- **Lists** return `{items, next_cursor}`. `limit` is 1 to 100, default 25.
  Pass `next_cursor` back as `cursor`; a `null` cursor ends the walk.
  Cursors are opaque: do not construct or edit them.
- **Errors** are RFC 9457 `application/problem+json`. Branch on `code`:

| code | status | meaning |
|---|---|---|
| `invalid_address` | 400 | not a Bech32m address (hex included) |
| `wrong_network` | 400 | a valid address for another network |
| `invalid_cursor` | 400 | cursor not issued by this API |
| `invalid_param` | 400 | malformed or out-of-range parameter |
| `not_found` | 404 | no indexed block or transaction under that reference |
| `no_staking` | 404 | the network has no validator set |
| `rate_limited` | 429 | too many requests; see `Retry-After` |
| `node_unavailable` | 503 | a live value needs the node, which did not answer |
| `index_behind` | 503 | the index cannot answer right now; retry |

## Headers

- `X-Indexed-Height`: highest indexed height.
- `X-Tip-Height`: the node's tip, when it answered.
- `X-Cache`: `HIT` or `MISS` on live data.
- `RateLimit-Limit`, `RateLimit-Remaining`, `RateLimit-Reset`, `Retry-After`: on 429.

## Caching

- A block or transaction at or below the finalized height is
  `Cache-Control: public, max-age=31536000, immutable`.
- Other history: `public, max-age=5`.
- Live data (balance, validators, supply, mempool, status): `no-cache` with an
  `ETag`. Send `If-None-Match` to get `304`.

## Rate limit

Default 10 requests per second per client, burst 30. The client is the socket
address by default. Behind a proxy, start the explorer with
`--api-client-ip cf` (Cloudflare) or `--api-client-ip xff` (a proxy that appends
to `X-Forwarded-For`). Only use a header mode when the proxy is the only route
in: a client can otherwise write its own header.

## Routes

| Route | Source | Notes |
|---|---|---|
| `GET /api/v1/status` | index + node | chain id, network, heights, lag, counts, average block time |
| `GET /api/v1/blocks` | index | newest first |
| `GET /api/v1/blocks/{height\|id}` | index | |
| `GET /api/v1/blocks/{height\|id}/transactions` | index | inclusion order, ascending |
| `GET /api/v1/transactions` | index | newest first |
| `GET /api/v1/transactions/{txid}` | index | includes `outputs` |
| `GET /api/v1/accounts/{address}` | node | balance and next nonce, live |
| `GET /api/v1/accounts/{address}/transactions` | index | `direction=sent\|received\|both` |
| `GET /api/v1/validators` | node | `no_staking` on a chain without staking |
| `GET /api/v1/supply` | node | cached 30 s: the node scans every account |
| `GET /api/v1/mempool` | node | `{size, bytes}` only |

## Example

```text
curl -s 'https://<explorer>/api/v1/accounts/<bech32m>/transactions?direction=received&limit=10'
```

## Deprecated

The unversioned `/api/*` routes (used by the explorer's own pages) still
answer. They carry `Deprecation: true` and a `Link` to `/api/v1/status`.
They have no stability promise.

## Not in v1

GraphQL, websockets for the API, and SDKs. The SDKs are a separate change.
