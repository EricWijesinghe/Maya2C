# ADR-152: Post-quantum peer authentication with static ML-KEM keys

- **Status:** accepted (design), 2026-11-24. The handshake core is built and tested, but the swarm does not use it yet (see Status).
- **Decision owner:** Eric, 2026-11-24 ("start working on the gap items"; gap register: ML-KEM pq-peer-auth, the active-impersonation gap).
- **Related:** ADR-022 (signer channel), ADR-033, `crates/node/src/network/pq/mod.rs` ("What it does not"), CATALOG row pq-peer-auth.

## Context

The P2P transport has two layers. libp2p-noise runs `Noise_XX_25519_ChaChaPoly_SHA256`, which authenticates the peer against its ed25519 `PeerId`. Inside that session, `/maya/mlkem1024/1.0.0` (with `/maya/mlkem/1.0.0` and the dual-KEM stage) adds post-quantum confidentiality from an **ephemeral** ML-KEM exchange. That stops harvest-now-decrypt-later. It does not stop an active attacker who can break ed25519 at the moment of the connection: that attacker can complete Noise as any peer, then run the ephemeral ML-KEM stage honestly, because the stage proves nothing about who is on the other end.

On a public gossip network most peers are strangers, so impersonating one of them gains little. Impersonation matters for peers someone has **pinned**: bootnodes, a validator's private links to other validators, sentry nodes, and any peer an operator lists by identity. Those are the links an eclipse or a vote-delay attack goes after.

## Decision

**A peer's post-quantum identity is a static ML-KEM-1024 key, and authentication is implicit through encapsulation (KEM-based authenticated key exchange, the KEMTLS / PQ-Noise `KK` shape). Peers do not sign anything.**

1. **Identity.** Every node holds a static ML-KEM-1024 key pair, derived from a 32-byte seed kept in a typed, zeroized wrapper, stored beside the libp2p identity and never logged. Its public identity is
   `pq_id = BLAKE3-derive-key("maya2c p2p pq identity v1", ek)`. A pinned peer is written `PeerId` + `pq_id`, for example `/p2p/<PeerId>/pq/<hex pq_id>` in a bootnode list.
2. **Handshake (inside the Noise session, after the ephemeral ML-KEM stage).**
   - Each side sends its static encapsulation key `ek_s`. The Noise session already encrypts the transcript, so an observer never sees which identity connected.
   - Each side checks the received `ek_s` against its pin when the peer is pinned. An unpinned peer is recorded trust-on-first-use against its `PeerId`, and a later `ek_s` that differs is refused.
   - Each side encapsulates a fresh secret to the *other* side's `ek_s` and sends the ciphertext.
   - Session keys = `KDF(ephemeral_ss ‖ ss_to_responder ‖ ss_to_initiator ‖ transcript)`, where the transcript covers both `PeerId`s, both static keys, the ephemeral exchange and both ciphertexts.
   - **Key confirmation.** Each side sends `MAC(k_confirm, transcript ‖ role)` and checks the other side's MAC before any application frame is sent. Only the holder of a static decapsulation key can derive `ss` for its side, so a wrong MAC means the peer is not the identity it claimed. The connection is closed and the peer is scored down.
3. **Downgrade.** The authenticated stage is negotiated as its own protocol, `/maya/mlkem1024-auth/1.0.0`. For a pinned peer, a node never falls back to the unauthenticated protocol. A pin is therefore also a statement that the peer must authenticate.
4. **Validators.** A validator's static p2p key can later be announced next to its consensus key, so that validator private links are pinned from chain state rather than from config. That needs a new staking-record field and its own ADR amendment. Until then, validators pin each other in config.

## Why KEM authentication, not signatures

- **Privacy.** A signature over the transcript is a transferable proof that a given identity took part in a given session. KEM authentication is deniable: each side can simulate the other side's messages, so nothing a peer keeps proves to a third party who it talked to. This supports the user-anonymity bar.
- **Cost.** One ML-KEM-1024 encapsulation and one decapsulation per side add 1,568 + 1,568 bytes each way. ML-DSA-87 would add a 2,592-byte key and a 4,627-byte signature per side, and signing costs more CPU. This ADR does not quote handshake times: they have not been measured (MISSING).
- **One primitive.** The stage reuses the FIPS 203 implementation the transport already runs and its known-answer tests (`kem_kat_tests.rs`), so no second post-quantum family enters the transport.

Signatures stay the right tool where a transferable proof is the point, such as consensus votes and the remote-signer channel (ADR-022), and those keep them.

## What it guarantees

A peer that holds the pinned static key cannot be impersonated, even by an attacker who can break X25519 and ed25519 at the moment of the connection: completing the handshake requires decapsulating under that key. Harvest-now-decrypt-later protection is unchanged.

## What it does not

- It does not authenticate peers nobody pinned on their first contact. Trust-on-first-use protects every later connection, not the first one.
- It leaves the libp2p `PeerId` classical. A node's routing identity in the DHT and in gossipsub scoring is still ed25519. Replacing it means changing libp2p itself and is out of scope.

## Rejected alternatives

- **ML-DSA signatures over the transcript.** Rejected for the privacy and size reasons above.
- **A post-quantum Noise pattern in libp2p-noise.** Not reachable without forking it (see `network/pq/mod.rs`).
- **Putting the static key inside the `PeerId`.** The multihash identity would become 1.5 KB and every libp2p tool would break.

## Status

- **Built:** the handshake core (`crates/crypto-pq/src/peer_auth.rs`, sans-IO): static identity, the transcript, the KDF, key confirmation and pin checks, with tests for a round trip, an impersonator who lacks the decapsulation key, a pin mismatch and a tampered ciphertext.
- **Not built:** wiring into the libp2p upgrade stack (`PqConfig`), the pin syntax in config and bootnode lists, the TOFU store, and announcing validator keys on chain. Class RESEARCH until the swarm uses it. Not consensus: the protocol is negotiated per connection, so it ships in any release without a fork.
