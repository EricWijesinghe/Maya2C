---
title: Wallet integration
description: Building and signing Maya2C transactions from an application.
---

Three facts decide the shape of any wallet integration here. Read them before
choosing a library.

## A signature is a hybrid pair, and it is large

Every transaction carries **two** signatures over the same payload:

| Scheme | Standard | Size |
|---|---|---|
| ML-DSA-65 | FIPS 204 (lattice) | 3,309 B |
| SLH-DSA-SHA2-128s | FIPS 205 (hash-based) | 7,856 B |
| | **total** | **11,165 B** |

Both must verify. Two families rather than one because a break in the lattice
assumption should not be a break in the chain — and hash-based signatures rest
on assumptions a quantum computer does not disturb.

The public key is 1,984 bytes. A transaction is therefore about 13 KB before it
carries any value, which is the constraint behind most of the decisions
downstream: block size, not compute, is the scarce resource on this chain.

## Signing is deterministic, and that is load-bearing

The same transfer signed twice is byte-identical. `crypto::keys` signs with
`try_sign_with_seed(&DETERMINISTIC_SEED, ..)` deliberately.

A transaction id hashes the signature. A hedged (randomised) signature would
therefore give one transaction **two identities** — the same transfer, submitted
twice, would look like two transfers to anything deduplicating by id.

For an integration this is good news:

- Re-signing and re-broadcasting is safe.
- An air-gapped signer restarted mid-scan produces the same QR frames, so a
  half-finished scan can be completed rather than restarted.

## The address domain is `custom-l1-node.address.v3`

An address is a domain-separated BLAKE3 hash of the hybrid public key. Deriving
one with any other domain string produces a well-formed address that **no key
can spend from** — funds sent there are gone, and nothing reports an error.

Do not reimplement the derivation. `sdk-wasm` is pinned against the node's own
implementation by `crates/node/tests/hybrid_parity_tests.rs` for exactly this reason: an
independent second implementation that drifts is indistinguishable from a
correct one until somebody loses money.

## Which package does what

```
maya2c.js      reads and submits, over the gateway.  No signing.
sdk-wasm       builds and signs transactions.        No network.
maya2c (PyPI)  signs.                                No network, no builder.
```

The split is deliberate. Signing needs the two post-quantum primitives, which
means compiled code — wasm in the browser, a native extension in Python.
Reading needs only HTTP.

```ts
import { MayaClient } from "maya2c.js";

const client = new MayaClient({ baseUrl: "https://gateway.example.com" });
const { balance, nonce } = await client.getBalance(address);

// ... build and sign with sdk-wasm, producing rawHex ...

const { txid } = await client.submitTransaction(rawHex);
```

## Talk to the gateway, not to a node

A node's JSON-RPC port has **no authentication** and serves
`get_mining_candidate` and `submit_block`. Pointing a client at it works for
reads and is the wrong thing to ship. See the [API reference](/guides/api).

## Nonces

`getBalance` returns the next valid nonce. Two transactions built against the
same nonce are two transactions of which the chain accepts one — the other is
silently not applied. If you submit in a burst, track the nonce locally and
advance it yourself rather than re-reading between submissions.

## Fees are burned

There is no coinbase mechanism, so a fee output goes to an unspendable address
(all zeros) rather than to a miner. Total supply is conserved; circulating
supply falls. `/v1/supply` reports both.
