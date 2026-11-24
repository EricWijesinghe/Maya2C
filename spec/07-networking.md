# 7. Networking — v0.1.0

Not consensus: two nodes that disagree here fail to talk, they do not fork.
Listed so a second client can interoperate. Node: `crates/node/src/network/`.

- **NET-1** Transport is libp2p over a Noise XX session (X25519, authenticating the peer against its `PeerId`), inside which an ML-KEM-768 (FIPS 203) exchange adds a secret a recording adversary cannot recover later: responder sends `version ‖ ek[1184]`, initiator replies `version ‖ ct[1088]`, and the derived keys bind the transcript. A dual-KEM mode adds HQC where the build includes it (node feature `hqc`).
- **NET-2** Gossip messages above 8 MiB are dropped.
- **NET-3** Blocks and transactions are gossiped in the encodings of §1 and §5.

## Node-local bounds (not interoperability rules)

A second client need not copy these; they are one node's resource limits
(ADR-147). Blocks whose parent is unknown are buffered up to 512 blocks and
64 MiB of wire bytes in all, at most 8 under one parent; a block over the
block byte cap, or whose body does not match its `tx_root`, is refused before
it is buffered. When the pool is full, a block a buffered orphan waits on
displaces the newest orphan nothing waits on. The block-sync request table and the RPC per-address
rate table evict their oldest entry at their ceiling rather than refusing a
newcomer or growing. Block-sync serving sends one peer at most 48 MiB and all
peers 256 MiB and 1,024 requests per 10 s window, a quarter of each kept for
peers the node dialed, over at most 8 streams per connection. The market HTTP server serves at most 256 live connections, closes
one whose request head takes longer than 10 s or that lives past 60 s, and
answers one non-loopback address at most 5 requests a second (burst 20).

## Gaps

No vectors; the handshake is covered by `crates/node/tests` and the KEM KATs.

## Node-local peer and RPC bounds (ADR-151)

Not interoperability rules: one node's limits, which a second client need not copy.
A gossiped block message whose bytes this node already accepted from gossip
is ignored rather than relayed again (keyed on the bytes, since a block id
does not commit to multisig approvals); gossipsub message ids stay author plus
sequence number. `/l1/bft` is scored like the other topics. A `/l1/bft` frame is
relayed only after every signature it carries verifies against its epoch's
committee and this chain's tag (a fetch, which carries none, when its sender id
is a member); one for an epoch the node cannot check is dropped, not relayed.
The node dials no private, loopback or link-local address unless started with
`--allow-private-peers`, and no DNS name the operator did not configure. At most 192 of
256 established connections are inbound, and one source (IPv4 address or
IPv6 /48, loopback exempt) holds at most 8 established inbound connections and
4 handshakes in flight; pinned bootnodes are exempt. A bootnode at a public or
DNS address must name `/p2p/<peer id>` unless the node is started with
`--allow-unpinned-bootnodes`. Post-quantum sessions are rotated oldest first,
at most 4 per sweep, once at least 10 sweeps old. JSON-RPC request bodies are
capped at two maximal transactions plus 64 KiB and must arrive within 30 s;
storage faults answer `-32603 "internal node error"`; public `get_peers` omits
direction, score and quarantine state; `threat_indicators` pages at most 256.
