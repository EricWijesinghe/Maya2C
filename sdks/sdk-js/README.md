# maya2c.js

TypeScript client for the Maya2C API gateway.

## Install

### Not published yet, build from source

`maya2c.js` is not on npm yet: `registry.npmjs.org/maya2c.js` answered 404 on
2026-11-13, and publishing waits for an explicit approval. Build it from a
clone and install the folder:

```bash
git clone https://github.com/EricWijesinghe/Maya2C
cd Maya2C/sdks/sdk-js
npm ci
npm run build                 # writes dist/, which package.json points at
cd /path/to/your/app
npm install /path/to/Maya2C/sdks/sdk-js
```

Node 20 or later. ESM only; there is no CommonJS build.

## What this talks to

The **gateway** (`maya-api-gateway`), not a node directly.

A node's JSON-RPC port has no authentication and serves `get_mining_candidate`
and `submit_block`. The gateway is the layer that makes a public client
possible: it has a default-deny allowlist that excludes both of those. Pointing
this client at a node would work for reads, and would be the wrong thing to
ship.

`cargo xtask kit up` starts a local devnet with a gateway on
`http://127.0.0.1:8080` ([testnet kit](../../docs/ecosystem/testnet-kit.md)).

## Send a transfer

This package reads and submits; it does not sign (see below). The transfer is
signed by `l1-wallet`, the Rust wallet shipped with the node, and submitted
from TypeScript. Against `cargo xtask kit up`, from the repository root:

```bash
export L1_WALLET_PASSWORD=devnet-only-password   # the devnet constant in xtask/src/devnet.rs
GENESIS=$(curl -s http://127.0.0.1:8080/v1/chain | sed -n 's/.*"genesis":"\([0-9a-f]*\)".*/\1/p')
target/debug/l1-wallet --keystore target/up/wallet.key --rpc-url http://127.0.0.1:32000 \
  send --yes --genesis "$GENESIS" --no-broadcast \
  --to 7777777777777777777777777777777777777777777777777777777777777777 --amount 500
# prints `raw: <hex>` and `txid: <hex>`; put the raw hex in RAW_TX
```

```ts
import { GatewayError, MayaClient } from "maya2c.js";

const client = new MayaClient("http://127.0.0.1:8080");
const recipient = "7777777777777777777777777777777777777777777777777777777777777777";

try {
  const { hash } = await client.sendRawTransaction(process.env.RAW_TX ?? "");
  console.log("accepted into the mempool:", hash);
  const { balance, nonce } = await client.getBalance(recipient);
  console.log(balance, nonce); // balance is a decimal string: amounts are 256-bit (ADR-046)
} catch (e) {
  if (e instanceof GatewayError) console.error(e.status, e.message);
  else throw e;
}
```

Acceptance is acceptance into a mempool, not inclusion in a block: poll the
balance or the block until it shows.

## Reading

```ts
const balance = await client.getBalance("a71cf0…"); // { balance: string, nonce: number }
const supply  = await client.getSupply();           // { circulating: string, total: string }
const block   = await client.getBlock(4200);
const health  = await client.health();
```

Every method rejects with a `GatewayError` carrying the HTTP status and the
gateway's message, rather than resolving to a shape you have to inspect.
`new MayaClient(baseUrl, { timeoutMs })` sets the request timeout (default
10 000 ms).

## Signing is not in this package

A Maya2C signature is a hybrid pair — ML-DSA-65 (FIPS 204) plus
SLH-DSA-SHA2-128s (FIPS 205), 11,165 bytes together, and **both must verify**.
Neither has a usable pure-JavaScript implementation, and a client that shipped
an approximate one would produce transactions the chain rejects, or worse,
accepts for the wrong reason.

Today transactions are signed by `l1-wallet`, as above. The `sdk-wasm` crate
(`sdks/sdk-wasm`) derives addresses and verifies hybrid signatures in the
browser; on master it does not build or sign transactions. Its construction is
pinned against the node's own implementation by `tests/hybrid_parity_tests.rs`,
so the two cannot drift into producing different addresses for the same key.

Two properties of that signing worth knowing before you build on it:

- **Signing is deterministic.** The same transfer signed twice is byte-identical.
  This is load-bearing rather than incidental: a transaction id hashes the
  signature, so a hedged signature would give one transaction two identities.
  It also means a re-signed transaction is safe to re-broadcast.
- **The address domain is `custom-l1-node.address.v3`.** Deriving an address with
  any other domain separator produces a well-formed address that no key can
  spend from.

## What is checked

- `npm test` (vitest): unit tests against a mock gateway, and the address
  vectors.
- `cargo xtask sdk-e2e` (default `--lang ts`) runs `test/live-node.test.ts`: a
  one-validator devnet and a real gateway, a transfer signed by `l1-wallet`,
  submitted with `sendRawTransaction`, and seen finalized. That is the flow in
  "Send a transfer". It runs when invoked locally; the nightly CI job runs the
  Python and Go live tests, not this one.
- `.github/workflows/sdks.yml` fails when an install line in this file names a
  package its registry does not have, unless the line sits under a "Not
  published yet" heading.
