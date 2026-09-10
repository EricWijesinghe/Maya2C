# maya2c.js

TypeScript client for the Maya2C API gateway.

```bash
npm install maya2c.js
```

## What this talks to

The **gateway** (`maya-api-gateway`), not a node directly.

A node's JSON-RPC port has no authentication and serves `get_mining_candidate`
and `submit_block`. The gateway is the layer that makes a public client
possible: it has a default-deny allowlist that excludes both of those. Pointing
this client at a node would work for reads, and would be the wrong thing to
ship.

## Reading

```ts
import { MayaClient } from "maya2c.js";

const client = new MayaClient({ baseUrl: "https://gateway.example.com" });

const balance = await client.getBalance("a71cf0…");
const supply  = await client.getSupply();
const block   = await client.getBlock(4200);
```

Every method rejects with a `GatewayError` carrying the HTTP status and the
gateway's message, rather than resolving to a shape you have to inspect.

## Signing is not in this package

A Maya2C signature is a hybrid pair — ML-DSA-65 (FIPS 204) plus
SLH-DSA-SHA2-128s (FIPS 205), 11,165 bytes together, and **both must verify**.
Neither has a usable pure-JavaScript implementation, and a client that shipped
an approximate one would produce transactions the chain rejects, or worse,
accepts for the wrong reason.

Use `@maya2c/sdk-wasm` (the `sdk-wasm` crate) to build and sign transactions,
then hand the resulting hex to `submitTransaction` here. `sdk-wasm`'s
construction is pinned against the node's own implementation by
`tests/hybrid_parity_tests.rs`, so the two cannot drift into producing different
addresses for the same key.

Two properties of that signing worth knowing before you build on it:

- **Signing is deterministic.** The same transfer signed twice is byte-identical.
  This is load-bearing rather than incidental: a transaction id hashes the
  signature, so a hedged signature would give one transaction two identities.
  It also means a re-signed transaction is safe to re-broadcast.
- **The address domain is `custom-l1-node.address.v3`.** Deriving an address with
  any other domain separator produces a well-formed address that no key can
  spend from.

## Submitting

```ts
const { txid } = await client.submitTransaction(rawHex);
```

The gateway forwards to a node and returns the transaction id. Acceptance into a
mempool is not inclusion in a block — poll for it.

## Compatibility

Node 20 or later. ESM only; there is no CommonJS build.
