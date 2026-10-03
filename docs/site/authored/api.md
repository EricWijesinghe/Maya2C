---
title: API reference
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
| `GET` | `/v1/fees` | Fee terms: `active`, `base_fee` per byte, and the `collector` the fee is paid to |
| `POST` | `/v1/transactions` | Submit a signed transaction |
| `POST` | `/v1/sealed` | Submit a sealed (encrypted-mempool) transaction |
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
# {"total":21000000000,"circulating":20999998400,"shielded":0,"burned":1600}
```

`total` and `circulating` differ because fees are burned to an unspendable
address rather than paid to a miner — the chain has no coinbase mechanism. Units
at that address still exist, so `total` is conserved and `circulating` falls.

`shielded` is the amount inside the shielded pool. It is included in
`circulating` and broken out because it is the figure an auditor most wants and
the one a transparent scan cannot recover.

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
