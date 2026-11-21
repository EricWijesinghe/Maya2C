# 7. Networking — v0.1.0

Not consensus: two nodes that disagree here fail to talk, they do not fork.
Listed so a second client can interoperate. Node: `crates/node/src/network/`.

- **NET-1** Transport is libp2p over a Noise XX session (X25519, authenticating the peer against its `PeerId`), inside which an ML-KEM-768 (FIPS 203) exchange adds a secret a recording adversary cannot recover later: responder sends `version ‖ ek[1184]`, initiator replies `version ‖ ct[1088]`, and the derived keys bind the transcript. A dual-KEM mode adds HQC where the build includes it (node feature `hqc`).
- **NET-2** Gossip messages above 8 MiB are dropped.
- **NET-3** Blocks and transactions are gossiped in the encodings of §1 and §5.

## Gaps

No vectors; the handshake is covered by `crates/node/tests` and the KEM KATs.

## Node-local peer and RPC bounds (ADR-151)

Not interoperability rules: one node's limits, which a second client need not copy.
A gossiped block whose header id this node already accepted from gossip is
ignored rather than relayed again; gossipsub message ids stay author plus
sequence number. `/l1/bft` is scored like the other topics. At most 192 of
256 established connections are inbound, and one source (IPv4 address or
IPv6 /48, loopback exempt) holds at most 8 established inbound connections and
4 handshakes in flight; pinned bootnodes are exempt. A bootnode at a public or
DNS address must name `/p2p/<peer id>` unless the node is started with
`--allow-unpinned-bootnodes`. Post-quantum sessions are rotated oldest first,
at most 4 per sweep, once at least 10 sweeps old. JSON-RPC request bodies are
capped at two maximal transactions plus 64 KiB and must arrive within 30 s;
storage faults answer `-32603 "internal node error"`; public `get_peers` omits
direction, score and quarantine state; `threat_indicators` pages at most 256.
