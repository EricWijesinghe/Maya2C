# 7. Networking — v0.1.0

Not consensus: two nodes that disagree here fail to talk, they do not fork.
Listed so a second client can interoperate. Node: `crates/node/src/network/`.

- **NET-1** Transport is libp2p over a Noise XX session (X25519, authenticating the peer against its `PeerId`), inside which an ML-KEM-768 (FIPS 203) exchange adds a secret a recording adversary cannot recover later: responder sends `version ‖ ek[1184]`, initiator replies `version ‖ ct[1088]`, and the derived keys bind the transcript. A dual-KEM mode adds HQC where the build includes it (node feature `hqc`).
- **NET-2** Gossip messages above 8 MiB are dropped.
- **NET-3** Blocks and transactions are gossiped in the encodings of §1 and §5.

## Gaps

No vectors; the handshake is covered by `crates/node/tests` and the KEM KATs.
