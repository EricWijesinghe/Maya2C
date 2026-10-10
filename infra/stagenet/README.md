# Staging net — many validators, one machine

`gen.sh` builds a throwaway BFT network where **each validator runs in its own
container**, with its own IP on a private Docker bridge and its own simulated
WAN link (`tc netem`, applied by `entrypoint.sh`). It exists to exercise the
node the way a real multi-host deployment would — peer discovery over distinct
addresses, latency, packet loss, partitions — on a single PC, before the Oracle
Cloud machines for gate 4 exist.

```sh
# inside WSL (or any Linux host with Docker), with Linux-built binaries:
infra/stagenet/gen.sh <bin-dir> <out-dir> [validators]   # default 7
docker compose -f <out-dir>/compose.yml up -d --build
# each node's RPC: http://172.30.0.(11+i):8545
docker compose -f <out-dir>/compose.yml down -v          # tear down
```

## What this is NOT

This is **one physical machine**. It is a networking, latency and
failure-handling rig, not a decentralisation claim.

- It is **not gate 4**. Gate 4 requires validators on separate machines run by
  separate operators; a container is neither. Nothing generated here may be
  described as independent, external, or as satisfying gate 4.
- A shared host shares a CPU scheduler, a clock and a kernel. Correlated
  failures that distinct machines would not have are possible here and are a
  property of the rig, not the protocol.

Record staging results as "staging net (N containers, one host)", never as a
validator count that implies separate operators.

## Files

| File | Role |
|---|---|
| `gen.sh` | Generates keys, `genesis.json`, per-validator dirs, and `compose.yml`. |
| `Dockerfile` | Runtime image around a host-built `maya2c-node`; adds `iproute2` for `tc`. |
| `entrypoint.sh` | Applies `$NETEM` to `eth0`, then drops to the unprivileged `l1` user. |

The binaries are **built on the host** (`cargo build --release -p maya2c-node
-p l1-wallet`) and copied in, so the image carries the exact artifact under
test. The base is `ubuntu:24.04` to match the build host's glibc.
