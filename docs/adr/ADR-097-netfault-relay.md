# ADR-097: Netfault relay — partition, latency and loss between validators

Status: Accepted (tooling only; no consensus, wire or state change)
Date: 2026-11-11

## Context

The localnet and attacknet runs test crashes, equivocation, garbage on the wire
and RPC floods. None of them can make a link misbehave. Every validator talks
to every other over loopback at full speed, so the test-gap review of
2026-11-10 found the largest gap to be the absence of any partition, latency or
packet-loss test. A BFT chain's safety under a split network and its liveness
under slow or lossy links are the claims that such a test would check, and
nothing in the tree checks them.

Two constraints shaped the design:

1. The node binary is frozen (AGENT-RULES, bb4e0940 until genesis). The harness
   must not need a flag in `maya2c-node`.
2. Validators learn each other's addresses through identify and Kademlia. A
   relay that only sits on the bootnode path can be bypassed if a node dials a
   peer's real port directly.

## Decision

`xtask/src/netfault.rs` is a TCP relay built on tokio, a dependency the
workspace already locks.

- **One listener per directed link.** Validator `from` dials the relay address
  for `to`; the relay forwards to `to`'s real p2p port. Listeners take port 0,
  and the bound addresses are handed to the nodes as bootnodes.
- **Bypass is closed by configuration, not by the relay.** The validators run
  without `--allow-private-peers`. The loopback addresses that identify
  announces are then filtered out of Kademlia (`crates/node/src/network/node.rs`,
  the `allow_private || is_routable` filter), so a node reaches a peer only
  through the bootnode address it was given. The partition scenario checks this
  by reading `get_peers` on both sides and failing if a cross-partition peer
  id appears.
- **Faults**, per link, in both directions:
  - partition into sides; a cut link refuses new connections and severs open
    ones; a scheduled heal restores it, and a stale heal timer cannot undo a
    newer partition (a generation counter);
  - fixed latency with uniform jitter, `latency ± jitter`, never negative,
    kept in FIFO order per direction (TCP never reorders);
  - loss as a retransmission stall. TCP never delivers a gap, so a dropped
    chunk is modelled as the retransmission timeout the application would see
    (`RTO` = 200 ms, the Linux minimum). The stream is never corrupted;
  - a bandwidth cap per direction.
- **Determinism.** Every fault decision comes from a splitmix64 stream seeded
  from the run seed, the link and the connection. A seed replays the same
  decisions. The OS scheduler still decides wall-clock timing, so a replay
  repeats the decisions, not the timings.
- **Two entry points**, one fault engine:
  - `cargo xtask netfault [--scenario partition|latency|loss|all] [--seed N]`
    runs four validators (the localnet) with the scenarios: a 2|2 partition
    must not finalise on either side and must heal to one chain; 300 ms ± 100 ms
    latency and 10% loss are timed with transfers, and the median is reported.
  - `cargo xtask attacknet --netfault latency|loss` relays the seven-validator
    attacknet's links with a profile for the whole run. The partition needs
    the four-validator command: a 2|2 split of seven seats leaves three seats
    on no quorum-capable side and tests a different claim.

## Alternatives considered

- **Kernel or firewall faults** (`tc netem`, Windows Filtering Platform). The
  stagenet already uses `tc netem` on a Linux host (see the stagenet memory
  note). Windows firewall rules need administrator rights and cannot shape
  latency per link. Rejected for this machine; the relay runs anywhere.
- **A `--p2p-advertise` or `--listen` flag in the node.** Would close the bypass
  more directly, but changes `crates/node`, which is frozen until genesis, and
  would have needed a post-genesis draft PR. The configuration route needs no
  node change.
- **Modelling loss as dropped bytes.** Corrupts the stream, so the node would
  see a protocol error instead of a slow link. The retransmission model is what
  an application sees on a lossy TCP path.
- **Adding `rand` to xtask.** `rand_chacha` is in the workspace, but splitmix64
  is about a dozen lines and needs no new crate in the tree.

## Consequences

- A partition run can now fail for a reason the node did not cause: a bypass.
  The `get_peers` check turns that into a visible failure, not a false pass.
- Loss is a liveness pressure in this model, not a model of packet loss. The
  claim tested is "finality survives a lossy path", not "the wire loses bytes".
- Run timings depend on this machine's load (one operator, loopback, a shared
  proving run). They are measured on this host, not a network figure. The
  reports say so.
- Nothing in consensus changes. `crates/node` is not touched.

## Verification

Unit tests in `xtask/src/netfault.rs` run in CI without a node: seeded
determinism, the jitter bound, the loss stall, the bandwidth arithmetic, the
partition sides, and in-process relay tests over real sockets (byte-exact
pass-through, stream integrity under loss, latency paid in each direction,
refusal and severing under partition, scheduled heal, and a stale timer).

The end-to-end scenarios need the localnet and a free machine. Their status is
recorded in `reports/netfault/` and in `features.toml` (`netfault-relay`).
