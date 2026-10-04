# Maya2C node-v0.1.0-testnet.2: join from anywhere, rejoin after anything

**Upgrade if you run a node on maya-testnet-1.** This release lets an
outside operator reach the network from behind NAT, join it in minutes, and
rejoin it after an outage. It is a testnet release: nothing on the chain
carries value.

> **Testnet, unaudited.** No external security audit has reported yet, and
> mainnet stays blocked until one has. The attacknet below is our own, run
> by one operator on one machine. It is a floor, not an independent test.

## Join in one command (Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/EricWijesinghe/Maya2C/master/scripts/join-testnet.sh | bash
```

The script:
- checks this release against `SHA256SUMS`;
- checks the genesis against a pinned hash;
- keeps your validator key on your machine (mode 600);
- installs a sandboxed systemd service that loads a snapshot and follows
  the chain.

The guide is at https://maya2c.dev/guides/validators/.

## What's new

### Network
- **WebSocket P2P.** The public bootnode is
  `/dns4/p2p.maya2c.dev/tcp/443/wss/p2p/12D3KooWK2bykmqyzdK4TMSTBQ5visSjkFoK8wj88aMn3ms4ByiG`.
  It works through NAT and firewalls that only allow HTTPS.
- **Snapshot bootstrap.** `--bootstrap-from https://bootstrap.maya2c.dev/rpc`
  restores a recent state instead of replaying every block. On 2026-10-04 a
  fresh node joined this way and kept pace with the tip.
- **Bootnodes are redialled** every 10 s while a node is short of peers.
- **Bootstrap retries** a rate-limited or unreachable peer with backoff,
  instead of giving up on the first refusal.

### Consensus and rejoin (found by the attacknet)
`cargo xtask attacknet` runs seven validator processes through six attacks,
round after round, while staking reshapes the committee:
- crash f, and crash f + 1;
- a stolen key signing from a second machine;
- random bytes on hundreds of p2p connections;
- an RPC flood;
- an outage longer than the engine's window.

It checks that no fork appears at any height. It found, and this release
fixes:
- **No rejoin across an epoch boundary.** Nodes now keep each recent
  epoch's final checkpoint (`get_checkpoint [epoch]`) and catch up one
  epoch at a time.
- **Restarted validators stranded as followers.** A node now follows
  attested blocks only when its own engine cannot derive the gap, and a
  builder whose engine is stuck is rescued.
- **One far-future attestation** from a single Byzantine member could wipe
  every pending checkpoint. Attestations are now height-bounded.

Checkpoints still need n − f signers. That threshold is deliberate: a node
catching up never trusts less than two thirds of the stake.

### Stake-weighted committees at genesis (ADR-040 part 2)
A genesis can weight committee votes and checkpoints by stake. Cheap
absent seats then cannot halt the chain. The mainnet ceremony turns this
on. maya-testnet-1 keeps equal votes, guarded by `--min-register-bond`.

### Infrastructure
- **status.maya2c.dev**: uptime and halt history, measured from the
  chain's own block timestamps.
- **bootstrap.maya2c.dev**: snapshots and headers for new nodes, separate
  from the public API.

## Evidence
- attacknet `--rounds 3`, five runs in a row: 15/15 rounds pass, no fork.
- custom-l1-node lib and BFT suites: 376 passed. api-gateway: 57/57.
- CI green on every merged change (#75–#85).

## Known and open
- More than f validators that each genuinely need to follow attested
  blocks cannot rejoin from checkpoints alone. Recovery is a snapshot
  restore (gate 10, ADR-038).
- Observer nodes follow by polling a peer's RPC rather than over p2p
  (KNOWN_ISSUES 20).
