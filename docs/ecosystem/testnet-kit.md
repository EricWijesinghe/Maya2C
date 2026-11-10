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
