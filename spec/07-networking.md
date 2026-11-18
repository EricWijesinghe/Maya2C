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
newcomer or growing. The market HTTP server serves at most 256 live
connections and closes one whose request head takes longer than 10 s.

## Gaps

No vectors; the handshake is covered by `crates/node/tests` and the KEM KATs.
