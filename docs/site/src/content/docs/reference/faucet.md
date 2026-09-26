---
title: 'The testnet faucet'
editUrl: false
# GENERATED from docs/faucet.md by scripts/ingest.mjs. Edit the source, not this.
---
A faucet is a hot wallet with a public endpoint. That is the whole security
story, and everything below is a consequence of it.

## It does not run where value is real

`Faucet::new` refuses `mainnet` and `maya-mainnet`, at construction rather than
per request, so a misconfigured deployment fails to start instead of failing on
the first request somebody is watching.

This is the sixth such guard, alongside `CIRCUIT_IS_AUDITED`, the node's startup
check, both terraform module sets, the genesis ceremony, and the wallet's
shielded composer. The list is repeated in each rather than shared, so that
relaxing one is a decision about one.

## Three controls, at three scopes

| Control | Bounds | Where |
|---|---|---|
| rate limiter | one actor, per IP **and** per address | `apps/faucet/src/limit.rs` |
| daily cap | the whole day, regardless of who asks | `apps/faucet/src/lib.rs` |
| chain refusal | value-bearing chains, outright | `apps/faucet/src/lib.rs` |

### Why the limiter has two independent buckets

Keying on the `(IP, address)` **pair** is not a limit. Generating an ML-DSA
keypair is free and unbounded, so one IP with a thousand fresh addresses is a
thousand distinct pairs and a thousand payouts. The mirror image holds: one
address behind a thousand proxies is a thousand pairs.

So the two buckets are independent and a request must clear both. Both are
consumed only if both pass — `check_and_commit` tests both before writing
either. Without that ordering, somebody who mistypes an address that was
already funded loses their own daily access through no fault of theirs.

A clock that has gone backwards refuses rather than granting twice. Refusing is
recoverable in a day; granting twice is not recoverable at all.

### Why the cap is the control that matters

A thousand IPs *and* a thousand fresh addresses is a thousand requests
indistinguishable from a thousand real users. No per-key limiter can see it.
The cap is what makes that attack cost a number somebody chose rather than
everything.

`apps/faucet/tests/load_tests.rs` is named for this. It fires 1,000 concurrent
requests and proves:

- one IP racing 1,000 requests gets exactly one grant, regardless of
  interleaving — the check and the commit are one step;
- however many callers arrive at once, the total dispensed never exceeds the
  cap;
- 999 refusals consume no budget, so a flood costs the faucet CPU and nothing
  else.

It does **not** prove that a distributed attacker cannot take the day's budget.
They can. The cap is the choice of how much that costs.

## Status codes

| Code | Meaning |
|---|---|
| `200` | funded; body carries the txid and what is left today |
| `400` | the address was not 64 hex characters |
| `429` | a rate limit, with `retry_after` and which limit was hit |
| `503` | the day's budget is spent, or the faucet account is empty |
| `500` | the node refused the transaction, or signing failed |

`429` and `503` are the service working. Only `500` means it is broken, so an
alert on 5xx is an alert about the faucet rather than about it being busy.

The two `429`s are distinguishable — `rate_limited_ip` and
`rate_limited_address` — because the remedies differ. Telling a user behind a
shared NAT that "this address already has funds" would be false.

## A failed submission does not refund the grant

If the node refuses the transaction after the windows have been recorded, the
caller gets a `500` and their day is still spent. That is deliberate: a
submission failure the caller could retry freely would be a way to spend the
budget without ever completing a grant. The operator sees the 5xx.

## The client's IP

Behind a load balancer every request arrives from the balancer, so keying on
the peer address would give the whole internet one shared bucket. Set
`MAYA_FAUCET_TRUST_PROXY=true` and the faucet reads `X-Forwarded-For` instead —
taking the **last** entry, not the first. A client can append entries; it
cannot remove the one the proxy appends.

Only set it when the proxy is the *sole* route to the port. If the port is also
reachable directly, the direct path has no rate limit at all.

## Running it

```bash
export MAYA_FAUCET_KEY=<4096 bytes of hex>     # never a flag: /proc/<pid>/cmdline
export MAYA_FAUCET_CHAIN=maya-genesis-rc1
export MAYA_FAUCET_NODE=http://127.0.0.1:8545
export MAYA_FAUCET_DISPENSE=1000
export MAYA_FAUCET_DAILY_CAP=1000000
cargo run -p maya-faucet --release
```

The key is read from the environment rather than a flag because a command line
is readable by every process on the host.

The nonce is read from the node once at startup and advanced locally. Reading
it per request would let two grants issued before the first is mined both take
the same nonce, and the chain would accept one — dropping a user's funding on
the floor while the faucet reported success. The cost is that a restart re-reads
it and a rejected submission leaves a gap; both are recoverable by restarting.
