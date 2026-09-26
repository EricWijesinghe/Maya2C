---
title: API reference
description: The gateway's REST surface, and why it is the only one you should call.
---

Two HTTP surfaces exist. Only one of them is for you.

| Service | Protocol | Public? |
|---|---|---|
| `maya-api-gateway` | REST | **Yes.** This is the surface. |
| node JSON-RPC | JSON-RPC 2.0 | **No.** Unauthenticated, firewalled to loopback. |

The node's RPC has no authentication and serves `get_mining_candidate` and
`submit_block`. Anything that reached it could take mining work and submit
blocks. The gateway exists to be the thing in front of it, with a **default-deny
allowlist** that excludes both — and it is the only component that terminates
TLS.

That is also why this page describes REST and not JSON-RPC. OpenAPI describes
resources at paths; JSON-RPC is one endpoint with a method named in the body,
and a generated client for it has a single untyped `post` function.

## Endpoints

| Method | Path | Returns |
|---|---|---|
| `GET` | `/health` | Gateway liveness. Says nothing about the node. |
| `GET` | `/v1/accounts/{address}` | Balance and next nonce |
| `GET` | `/v1/blocks/{height}` | A block |
| `GET` | `/v1/supply` | Circulating and total supply |
| `POST` | `/v1/transactions` | Submit a signed transaction |
| `POST` | `/v1/sealed` | Submit a sealed (encrypted-mempool) transaction |

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

Build and sign with `sdk-wasm`; see [wallet integration](/guides/wallet). This
API never sees a private key.

## Rate limits

Per-IP token bucket. Behind a load balancer the gateway must be told to read the
forwarded header, and it takes the **last** hop rather than the first — a client
can append entries to `X-Forwarded-For`, but cannot remove the one the proxy in
front appends.
