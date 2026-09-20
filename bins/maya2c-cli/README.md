# bins/maya2c-cli

Empty. There is no `maya2c-cli`.

The foundation brief lists it under `bins/`. The brief also says not to build
product features in this phase, so this is the reserved place rather than the
thing.

Before one is written, the question worth settling is what it would be *for*,
because the operator-facing work is already split across five executables and
a CLI that wrapped them would be a sixth name for the same actions:

| Binary | What it does |
|---|---|
| `bins/maya2c-node` | runs the validating node |
| `bins/maya2c-miner` | CPU reference miner |
| `bins/maya2c-gpu-miner` | the same work on a GPU |
| `bins/maya2c-genesis` | writes a genesis file |
| `bins/genesis-ceremony` | the multi-party genesis key ceremony |
| `bins/maya2c-peerid` | prints the libp2p peer id for an identity file |
| `bins/l1-wallet` | keystore, transaction construction, RPC submission |

The gap a CLI would actually fill is *querying and operating a running node* —
today that means hand-written JSON-RPC calls against the six methods
`crates/node/src/rpc/` exposes, or the REST and GraphQL surface in
`crates/api-gateway`. A CLI over `api-gateway` would inherit its schema and
its tests; one with its own hand-rolled request types would drift from them.

`xtask` is deliberately not that tool. It automates the *workspace* —
`cargo xtask disk`, `cargo xtask coverage` — and has no chain dependency, which
is why it can run before anything else compiles.
