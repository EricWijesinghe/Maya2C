---
title: API reference
head:
  - tag: title
    content: "Maya2C API Reference: JSON-RPC and REST Endpoints"
description: The gateway's REST surface, and why it is the only one you should call.
---

Two HTTP surfaces exist. Only one of them is for you.

| Service | Protocol | Public? |
|---|---|---|
| `maya-api-gateway` | REST, and JSON-RPC 2.0 at `/rpc` | **Yes.** This is the surface. |
| node JSON-RPC | JSON-RPC 2.0 | **No.** Unauthenticated, firewalled to loopback. |

The node's RPC has no authentication and serves `get_mining_candidate` and
`submit_block`. Anything that reached it could take mining work and submit
blocks. The gateway exists to be the thing in front of it, with a **default-deny
allowlist** that excludes both — and it is the only component that terminates
TLS.

This page mostly describes REST, because OpenAPI describes resources at paths.
The gateway also answers JSON-RPC at `/rpc`, for clients that already speak the
node's protocol — `l1-wallet`, the desktop wallet, and the Go and Python SDKs.
It is not a proxy: each method below is parsed and checked by the gateway, and
anything else is refused before it reaches the node.

The public testnet's gateway will be `https://rpc.maya2c.dev` once the testnet
is live. `gateway.example.com` below stands for whichever gateway you use.

## Endpoints

| Method | Path | Returns |
|---|---|---|
| `GET` | `/health` | Gateway liveness. Says nothing about the node. |
| `GET` | `/v1/accounts/{address}` | Balance and next nonce |
| `GET` | `/v1/blocks/{height}` | A block |
| `GET` | `/v1/supply` | Circulating and total supply |
| `GET` | `/v1/supply/circulating` | Circulating supply in whole coins, plain text (for listing sites) |
| `GET` | `/v1/supply/total` | Total supply in whole coins, plain text |
| `GET` | `/v1/fees` | Fee terms: `active`, `base_fee` per byte, and the `collector` the fee is paid to |
| `POST` | `/v1/transactions` | Submit a signed transaction |
| `POST` | `/v1/sealed` | Submit a sealed (encrypted-mempool) transaction |
| `GET` | `/v1/subscribe` | WebSocket feed of blocks and transactions; see [event subscriptions](#event-subscriptions) |
| `POST` | `/v1/webhooks` | Register a webhook; `GET`/`DELETE` `/v1/webhooks/{id}` with its secret |
| `POST` | `/rpc` | JSON-RPC 2.0: `get_balance`, `get_block_by_height`, `get_fee_info`, `get_supply`, `send_raw_transaction`. Batches are refused. |

The machine-readable document is served at **`/openapi.json`** by the gateway
itself, rather than only committed here — a spec that lives in a repository and
not at the endpoint is a spec that drifts from the deployment somebody is
actually pointing a client at.

```bash
curl https://gateway.example.com/openapi.json | jq .
```

## `/health` does not proxy

Deliberately. A health check that reported the node's state would make an
orchestrator restart the gateway whenever the node was unwell — killing the one
component still able to return a useful error.

## Status codes

| Code | Meaning |
|---|---|
| `400` | Malformed input. The address was not 64 hex characters; the body was empty. |
| `404` | No block at that height. |
| `502` | The node refused it, or could not be reached. |

A `502` never carries the node's address or an internal path. An error message
that names your backend is a free map of the deployment.

## Reading

```bash
curl https://gateway.example.com/v1/accounts/a71cf0…
# {"balance":8400000000,"nonce":3}

curl https://gateway.example.com/v1/supply
# {"circulating":"20999998400000000000000000000000","total":"21000000000000000000000000000000"}
```

Both figures are decimal strings of base units: amounts are 256-bit
([ADR-046](https://github.com/EricWijesinghe/Maya2C/blob/master/docs/adr/ADR-046-universe-scale-supply.md)),
wider than a JSON number can carry exactly. The values above show the shape,
not real chain figures.

`total` counts every unit that exists, including units that can never move;
`circulating` counts the units that can. For the breakdown, ask the node's
`get_supply` JSON-RPC method: it also reports `shielded` (units inside the
shielded pool, already included in `circulating`) and `burned` (units at
addresses that can never spend them). The gateway's `/v1/supply` returns only
the two headline figures.

## Submitting

```bash
curl -X POST https://gateway.example.com/v1/transactions \
  -H 'content-type: application/json' \
  -d '{"raw":"0a1b2c…"}'
# {"hash":"9e6661…"}
```

Acceptance is acceptance **into a mempool** — not inclusion in a block. Poll the
account's nonce, or watch for the block.

**A transfer must pay a fee** where the chain charges one, and on the testnet it
does (ADR-029). Read `GET /v1/fees`, then add an output of at least
`base_fee × (signed size in bytes)` to the `collector` address. Anything above
that minimum is a tip to validators. `l1-wallet` and the desktop wallet do this
for you:

```bash
l1-wallet --rpc-url https://gateway.example.com/rpc send --to <address> --amount 500
```

Measured end to end on 2026-09-29 through a local gateway on a fee-charging
chain: a hybrid-signed transfer was 13,255 bytes, so the minimum fee at base fee
1 was 13,255, and the wallets' default ("Standard") paid twice that, 26,510.

Build and sign with `sdk-wasm`; see [wallet integration](/guides/wallet). This
API never sees a private key.

## Rate limits

A token bucket per client address: by default **40 requests at once, then 20
per second**. Over the limit the gateway answers `429 Too Many Requests`
with a `retry-after` header; back off and retry.

Operators choose how a client is identified (`--client-ip`). Directly exposed,
it is the TCP peer (`socket`). Behind Cloudflare, it is `CF-Connecting-IP`,
which Cloudflare's edge overwrites (`cf-connecting-ip`; this is how the public
testnet runs). Behind another reverse proxy, it is the **last**
`X-Forwarded-For` entry (`x-forwarded-for-last`): a client can prepend entries
to that header but cannot remove the one the proxy in front appends. Set
`--rate-per-second` and `--rate-burst` to change the limits.

## Event subscriptions

New blocks and transactions, filtered by `address` (sender or recipient, 64
hex characters) and/or `asset`. With no filter you get everything. A block event
matches if any of its transactions does.

```json
{"id":"tx:9e6661…","kind":"tx","height":1204,"data":{"txid":"9e6661…","sender":"a71cf0…","outputs":[…]}}
{"id":"block:1204","kind":"block","height":1204,"data":{"height":1204,"header":{…},"tx_count":3}}
```

`id` is what to deduplicate on. A transaction larger than 64 KiB arrives as
`{"txid":…,"truncated":true}`; fetch the block for the rest. The node's JSON
names no asset today, so no asset filter but `asset=native` (the chain's coin)
matches anything yet.

The feed starts at the tip the gateway sees when it starts: there is no replay
of history, and subscriptions live in the gateway's memory, so a gateway
restart drops them. Re-register on a `404`.

### WebSocket

```bash
websocat 'wss://gateway.example.com/v1/subscribe?address=a71cf0…'
```

A slow client skips events rather than holding the feed back, the same rule as
the explorer's `/ws/blocks`. At most 5 open streams per client.

### Webhooks

```bash
curl -X POST https://gateway.example.com/v1/webhooks \
  -H 'content-type: application/json' \
  -d '{"url":"https://hooks.example.com/maya","address":"a71cf0…"}'
# 201 {"id":"3f2a…","secret":"<64 hex>"}
```

**The secret is shown once.** Keep it; it signs every delivery and is what lets
you read or delete the subscription:

```bash
curl -H "authorization: Bearer $SECRET" https://gateway.example.com/v1/webhooks/3f2a…
# {"state":"active","delivered":12,"failed_attempts":1,"skipped":0,"last_error":…,"dead_letter":null,…}
curl -X DELETE -H "authorization: Bearer $SECRET" https://gateway.example.com/v1/webhooks/3f2a…
```

Each delivery is a `POST` of `{"subscription":id,"event":{…}}` with header
`x-maya2c-signature: sha256=<hex HMAC-SHA256 of the raw body under the secret>`.
Verify it over the exact bytes you received, in constant time, before trusting
the body (`maya_api_gateway::webhooks::verify` does this in Rust). The
signature covers the body only, with no timestamp: a replayed delivery repeats
an event `id` you have already seen, so drop it.

The gateway does not yet confirm that you own the URL you register, so a
caller can point deliveries at a third party's public endpoint. The caps below
bound how much; an ownership handshake is not built.

Any status other than 2xx is a failure. A failed delivery is retried with
exponential backoff (1 s, doubling, at most 60 s between tries; 5 attempts).
After the last attempt the subscription is **dead-lettered**: `state` becomes
`dead_letter`, `dead_letter.event` names the event that failed, and nothing more
is sent. Delete it and register again once the receiver is fixed.

Targets the gateway refuses:

- anything but `https://` (operators can allow `http://` for development with
  `--webhooks-allow-http`; the address rules below still apply);
- URLs with a username or password, or longer than 2,048 bytes;
- loopback, private, link-local, carrier-grade NAT, multicast, documentation
  and reserved addresses, IPv4-mapped and NAT64 forms of them included;
- a name that resolves to any such address. The name is resolved again before
  every delivery and the connection goes to the address that was checked, so a
  DNS record changed afterwards cannot point a delivery inside the operator's
  network. Redirects are not followed.

Limits: 5 webhooks per client (dead-lettered ones count until deleted), 1,000
per gateway, a 4 KiB registration body; over a limit is `429`.
