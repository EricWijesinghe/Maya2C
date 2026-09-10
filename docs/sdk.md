# SDKs and the API gateway

Four pieces: a gateway in front of the node, uniffi bindings for Python/Kotlin/
Swift, a wasm build for browsers, and a TypeScript client.

---

## What is verified, and what is only generated

The honest matrix first, because it is the part most easily overstated.

| Component | Built | Tested | Where |
|---|---|---|---|
| `api-gateway` (REST + GraphQL) | yes | **13 tests** | `api-gateway/tests/gateway_tests.rs` |
| `sdk-ffi` Rust core | yes | **9 tests** | `sdk-ffi/src/lib.rs` |
| Python bindings | yes | **9 tests, real cdylib** | `sdk-ffi/bindings/python/test_bindings.py` |
| Kotlin bindings | generated only | **no** | no `kotlinc` on the build host |
| Swift bindings | generated only | **no** | no Swift toolchain on the build host |
| `sdk-wasm` core | yes | **2 + 8 parity** | `sdk-wasm/src/lib.rs`, `tests/hybrid_parity_tests.rs` |
| `maya2c.js` | yes | **19 E2E tests** | `sdk-js/test/e2e.test.ts` |

uniffi emits Kotlin and Swift as text without needing those toolchains. Emitting
is not the same as working, and a generated-but-never-compiled binding is not a
working SDK. Both are shipped labelled, not claimed.

To close the gap: a CI runner with `kotlinc` and a macOS runner with Swift,
running the equivalent of the Python suite.

---

## 1. The gateway

The node's JSON-RPC port has **no authentication** — sound for something bound
to a pod network, unsound to expose. The gateway is what makes a public
interface possible.

### Default-deny allowlist

| Method | Proxied? |
|---|---|
| `get_balance`, `get_block_by_height`, `get_supply` | yes |
| `send_raw_transaction` | yes — a public chain is for this |
| **`get_mining_candidate`** | **no** |
| **`submit_block`** | **no** |

The miner methods are named in `allowlist::DENIED_METHODS` rather than merely
absent, so the exclusion has a test behind it. `get_mining_candidate` hands out
the header a miner is searching; `submit_block` makes every node on the network
do full block validation on demand. Both stay on the node's own port, to the
operator already inside the trust boundary.

The check lives in `NodeClient::call` — the single point every outbound call
passes — not at each route. A route that forgot the check would be a hole, and
the compiler cannot see a missing call to a free function.

### GraphQL limits are not optional

`schema()` installs a depth limit of 6 and a complexity limit of 200, and there
is **no constructor without them**. A `schema_unlimited` for tests would be the
version somebody eventually wired into `main`.

GraphQL lets a client compose its own query; over RocksDB that is an unbounded
work surface that ships enabled. No mutations are exposed — writes go through
REST, where the size ceilings and sealed validation live, and a mutation would
be a second write path that drifts from them.

### Sealed submission

The gateway validates shape — hex, length, non-empty — and forwards. It **never
decrypts** and holds no committee share. A gateway that could read the
threshold-encrypted mempool would be the observer that mempool exists to remove.

`MAX_SEALED_PAYLOAD_BYTES` (64 KiB) is checked against the *string* before
`hex::decode` allocates, so a hostile length never becomes a hostile allocation.

### Errors do not echo the backend

`GatewayError::Upstream` carries the node's message to the log and renders as a
flat `"upstream node error"` to the caller. Echoing it would publish the node's
internal address, paths, and versions to anyone who could provoke a failure.

---

## 2. uniffi bindings

### A secret key never crosses the FFI boundary

`HybridSigningKey` is `Zeroizing` — wiped on drop. That guarantee ends at the
boundary. In Python the secret becomes a `bytes` object: immutable, freely
copied by the interpreter, resident until GC, impossible to wipe. Kotlin and
Swift are no better.

So `SigningKey` is an **opaque handle**. Its entire public surface is
`from_seed`, `generate`, `public_key`, `address`, `sign`. There is deliberately
no `secret_bytes()`, and `test_there_is_no_way_to_export_the_secret` pins that
surface exactly — a keyword filter would flag `from_seed`, which is a
constructor (secret *entering*, which is fine), and miss an exporter named
`export` or `raw`.

### The cdylib links RocksDB

`HybridSigningKey` lives in `custom-l1-node`, which links RocksDB, so the
release cdylib is **5.0 MB** with a database embedded in it. That is a real cost
for a mobile SDK, and the structural fix is to extract `hybrid` into its own
crate. That refactor touches a consensus path and is deliberately not bundled
into an SDK change.

---

## 3. The browser SDK, and the duplication in it

`sdk-wasm` **cannot** depend on `custom-l1-node`: RocksDB is C++, and C++ does
not target `wasm32-unknown-unknown`. There is no feature flag that removes it,
because the node *is* the database.

So the hybrid construction is composed there from the same two primitives the
node composes it from — `fips204` and `maya-crypto-pq`, both pure Rust. **That
is duplication of a consensus-critical encoding.**

Duplication is dangerous when undetected, so it is detected.
`tests/hybrid_parity_tests.rs` signs with the node and verifies with the SDK,
derives addresses both ways, and checks that a forged half is refused in each
direction. Eight tests.

**It has already caught one error.** The SDK's `ADDRESS_DOMAIN` was written as
`maya-address-v3`; it is actually `custom-l1-node.address.v3`. That mistake
would have produced plausible-looking addresses that no key could ever spend —
a user would fund one and lose the funds.

### `verify_ml_dsa` is not enough to accept a transaction

It checks 3,309 bytes instead of 11,165, so a UI can show a provisional result
while the full check runs. Anything that decides to **spend** must use
`verify_hybrid`. Checking only the lattice half would accept exactly the forgery
the hybrid scheme exists to prevent.

---

## 4. The TypeScript client

`maya2c.js` talks to the **gateway**, not a node. Pointing it at a node would
work for reads and would be the wrong thing to ship.

No signature scheme is reimplemented in TypeScript. There is already one
hand-maintained copy of the hybrid encoding — see the parity test — and one is
the most this project should have.

### E2E tests use a real server

19 tests, each starting a mock gateway on an ephemeral port and speaking real
HTTP. Nothing is stubbed: a stubbed `fetch` tests that the client calls the
function it was given, and skips URL construction, JSON handling, status
mapping, and the abort path — which is most of what a client is.

Two behaviours worth naming:

- **4xx vs 5xx.** `GatewayError.retryable` is `status >= 500`. A client that
  collapsed both into a string would retry malformed requests forever.
- **GraphQL 200-with-errors.** GraphQL returns HTTP 200 and an `errors` array.
  A client checking only the status treats a rejected query — including one
  refused by the complexity limit — as a success with `data: null`. That is the
  single most common GraphQL client bug, and it is tested.

---

## Not built

- **New `StateDB` read paths.** The node exposes six RPC methods; dex, oracle,
  governance, sealed-mempool and shielded state have no RPC surface at all. The
  GraphQL schema serves what the node serves, and nothing more. Adding those
  read paths is the bulk of a richer query API.
- **No "DAG state" query, deliberately.** `crypto::dag` is the memory-hard
  proof-of-work *dataset*, not a block graph, and it is not chain state. The
  block-graph work in `maya-blockgraph` is a research branch no consensus path
  reaches. There is nothing to query, and a field returning something plausible
  would be worse than its absence.
- **No measured wasm signing figures.** `wasm-pack` is not installed on this
  host, so the `.wasm` artifact was not produced and browser signing time and
  bundle size are unmeasured. Expect materially worse than native — there is no
  AVX2 and no SHA extensions in wasm — and measure before promising a UX.
