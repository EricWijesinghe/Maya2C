# Testnet kit

One command starts a local devnet with its endpoints, and one command runs an
attack against a throwaway network. Everything listens on loopback, and every
key is fresh. Nothing here is a deployment.

## Run the devnet

```sh
cargo xtask kit up
```

It builds the binaries, starts four validators, the API gateway, the chat
relay, the faucet and the explorer, waits until the validators agree on a
chain, and prints the URLs and the funded wallet's keystore path. The
keystore's password is the devnet constant `PASSWORD` in `xtask/src/devnet.rs`.

```sh
cargo xtask kit status    # which endpoints answer right now
cargo xtask kit down      # stop what `up` started (stops only the PIDs it recorded, and only when their start times still match)
```

`up` refuses to start when a validator or service port is taken, so it will
not dial a live network on the same machine.

Time from a fresh clone to a running devnet: MISSING (no recorded run in
`reports/`; most of it is the first build).

### What it leaves running

Read from `xtask/src/up.rs`, `localnet.rs` and `devnet.rs`; `target/up/up.json`
holds the same, for scripts.

| What | Where |
|---|---|
| Validator JSON-RPC | `http://127.0.0.1:32000` (node 0) to `:32003` |
| API gateway (REST `/v1`, `/rpc`, `/openapi.json`) | `http://127.0.0.1:8080`, in front of node 0 |
| Faucet | `http://127.0.0.1:8090` (`POST /request {"address": "<hex>"}`) |
| Explorer | `http://127.0.0.1:3000` |
| Chat relay | `/ip4/127.0.0.1/tcp/4001` |
| Binaries | `target/debug/` (or `$MAYA2C_BIN_DIR` when set; then nothing is built) |
| Funded wallet keystore | `target/up/wallet.key`, password `devnet-only-password` |

The summary line prints the keystore as `target/up/wallet`; the file is
`target/up/wallet.key`, the path `devnet::wallet` writes.

A first transfer against it, from the shell or from code:
[quickstart](https://maya2c.dev/guides/quickstart/) and
[SDK wizard](https://maya2c.dev/guides/sdk-wizard/). `l1-wallet` signs only for
a genesis id it is given (`--genesis`, ADR-036); `get_chain_info` on any node
returns it.

For a node that stays (your own machine, a config file, a service definition),
`cargo xtask node setup --genesis FILE [--dir DIR]` checks the host and writes
the config, and `cargo xtask node doctor [--dir DIR]` checks a running node.
Neither installs anything.

## Attack a throwaway network

```sh
cargo xtask kit attack list                 # the scenario names
cargo xtask kit attack stolen-key           # one attack, on its own seven-validator network
cargo xtask kit attack crash-f --weighted   # stake-weighted variant
```

Each run checks that no fork appears anywhere and writes a dated report under
`reports/attacknet/`. `kit attack` refuses to run while `kit up` is running,
because both use the RPC ports. Stop the devnet first.

The full sequence (every attack in order, several rounds) is
`cargo xtask attacknet --rounds N`.
