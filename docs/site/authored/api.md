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

The public testnet is paused (since 2026-10-28), so there is no public gateway
to call today. `cargo xtask kit up` starts a local one on
`http://127.0.0.1:8080`, in front of a four-validator devnet
([quickstart](/guides/quickstart/)). `gateway.example.com` below stands for
whichever gateway you use. Client libraries: [SDK wizard](/guides/sdk-wizard/).

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
| `GET` | `/v1/chain` | The genesis id a transaction must be signed for (ADR-036), the chain id, and the tip height |
| `POST` | `/v1/transactions` | Submit a signed transaction |
| `POST` | `/v1/sealed` | Submit a sealed (encrypted-mempool) transaction |
| `POST` | `/graphql` | GraphQL reads: `account`, `supply`, `block` |
| `POST` | `/rpc` | JSON-RPC 2.0: `get_balance`, `get_block_by_height`, `get_fee_info`, `get_chain_info`, `get_bft_status`, `get_checkpoint`, `get_supply`, `get_mining_work`, `send_raw_transaction`. Batches are refused. |
| `GET` | `/openapi.json` | The OpenAPI document for the REST routes above |

## OpenAPI

The machine-readable document is served at **`/openapi.json`** by the gateway
itself, rather than only committed here — a spec that lives in a repository and
not at the endpoint is a spec that drifts from the deployment somebody is
actually pointing a client at. It is generated from the route handlers
(`crates/api-gateway/src/rest.rs`, utoipa), and `tests/openapi_tests.rs` fails
when a route the router registers is missing from it.

```bash
curl https://gateway.example.com/openapi.json | jq .
curl http://127.0.0.1:8080/openapi.json            # the gateway `cargo xtask kit up` starts
```

**Not on master yet:** a separate, versioned indexer API for the explorer, with
its own OpenAPI file (`apps/explorer/openapi/v1.yaml`, ADR-092), is in open
pull request [#414](https://github.com/EricWijesinghe/Maya2C/pull/414). Until it
merges, nothing on this page describes it.

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

Build and sign with `l1-wallet send --no-broadcast`, which prints the signed
hex; see [wallet integration](/guides/wallet). This API never sees a private
key.

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

## Node JSON-RPC methods

Every method the node registers on master, read from
`crates/node/src/rpc/server.rs`, `bootstrap.rs` and `peers.rs` (commit
`c7ebb4d2`). These are the node's own port: loopback by default, no
authentication. The **Gateway** column says what a client can reach through
`/rpc` (`crates/api-gateway/src/allowlist.rs`): *yes*, *bootstrap* (only on a
gateway started with `--bootstrap`), *denied* (named in the deny list), or
*no* (not forwarded). Params are a JSON array; addresses are 64 hex characters.

| Method | Params | Returns | Gateway |
|---|---|---|---|
| `get_balance` | `[address]` | Balance and next nonce | yes |
| `get_account_at_tip` | `[address]` | The account with the tip height and block id it was read at | no |
| `get_balance_at_height` | `[address, height]` | The balance as of a past height | no |
| `get_balance_changes` | `[height]` | The balances one block changed | no |
| `vault_get` | `[address]` | A vault account's settings, or `null` (ADR-030) | no |
| `get_chain_info` | `[]` | Genesis id, chain id, tip height | yes |
| `get_node_version` | `[]` | `version`, `protocol`, `header_version`, for upgrade checks | no |
| `get_bft_status` | `[]` | Epoch, round, committed round, committee size, this node's index; `null` off DAG-BFT | yes |
| `get_checkpoint` | `[]` or `[epoch]` | The newest checkpoint, or an epoch's final one | yes |
| `get_certificates` | `[epoch, offset, max]` | One bounded page of an epoch's certificate log (ADR-052) | no |
| `get_validators` | `[]` | Registered validators and this epoch's committee | no |
| `get_peers` | `[]` | Connected peers | no |
| `get_fee_info` | `[]` | `active`, `base_fee` per byte, `collector` | yes |
| `get_supply` | `[]` | Circulating, total, `shielded`, `burned` | yes |
| `get_mempool_info` | `[]` | `size` and `bytes` of the pool | no |
| `send_raw_transaction` | `[raw_hex]` | The transaction hash, once the mempool accepts it | yes |
| `get_block_by_height` | `[height]` | A block | yes |
| `get_blocks_by_height_range` | `[from, count]` | Consecutive blocks, stopping at the first missing one | no |
| `get_mining_work` | `[miner_address]` | A share header bound to that payout address (ADR-044) | yes |
| `get_mining_candidate` | `[]` | A full block template | denied |
| `submit_block` | `[raw_hex]` | Whether a mined block was accepted | denied |
| `get_tip_height` | `[]` | The tip height | bootstrap |
| `get_headers` | `[from, to]` | A bounded run of headers | bootstrap |
| `get_snapshot_manifest` | `[]` | The newest state snapshot's manifest | bootstrap |
| `get_snapshot_chunk` | `[height, index]` | One snapshot chunk | bootstrap |
| `htlc_get_lock` | `[lock_id]` | A hash-time-locked contract's state | no |
| `stateless_transaction_witness` | `[raw_hex]` | Research branch: refused until stateless accounts activate, which is no network today | no |
| `threat_indicators` | `[]` | Every threat-intel indicator, judged at the tip | no |
| `threat_peer_addresses` | `[]` | Connected peers' ids and IPs, for the threat-intel tooling | no |
| `iot_device` | `[device_id]` | An IoT anchor device's status and latest batch, never readings | no |

A method on an open pull request and not on master is not listed; this table
changes when one merges.
