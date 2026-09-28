# Prior art: a post-quantum peer-to-peer messenger (Maya Chat)

**Search status: partially searched 2026-09-28.** Web search of each project
named in the operating brief, plus Signal and iMessage, which the brief did
not name but which decide the answer. Status and Waku were not resolved.
This file authorizes **no** "first", "only" or "unprecedented" claim.

## What each does today

| Project | Architecture | Post-quantum E2EE | Where it struggles |
|---|---|---|---|
| Signal | Central servers, phone number (usernames optional) | **Yes.** PQXDH (2023); SPQR "triple ratchet" with ML-KEM-768, released 2025-10-02 | Phone number to register; a single operator |
| iMessage | Apple servers | **Yes.** PQ3 (2024) | Apple devices only |
| SimpleX Chat | Relays with no user identifiers; queue addresses per contact | **Yes, on by default** in direct chats since v5.6 (2024-03): sntrup761 hybridised into the double ratchet | Groups: sntrup761 keygen is slow, so groups > 10–20 members are not PQ; no long-lived identity to anchor |
| Session | Onion-routed swarm of service nodes, staking token | **Designing.** Protocol V2 (ML-KEM, PFS) announced; detailed specs expected in 2026 | Removed forward secrecy in its first protocol; V2 not shipped |
| Matrix | Federated homeservers | **No** in the main protocol (Olm/Megolm). A PQC wrapper for vodozemac exists for one government deployment | Metadata on homeservers; complex key backup |
| Briar | Direct P2P over Tor, Wi-Fi, Bluetooth | **No** evidence found | In maintenance mode (2026 announcement); Android only |
| Utopia P2P | Proprietary P2P network, built-in token | **No.** Curve25519 + AES-256 | Closed source; bundled token |
| Status | Waku (libp2p gossip) | **Not resolved** by this search | — |

## What this means for Maya Chat

"Post-quantum end-to-end encryption from day one" is **not** a difference:
Signal, iMessage and SimpleX all ship it today. Saying otherwise would break
Standing Order 10.

Candidate differences that are **not yet verified** (each needs its own
dated search before it can be claimed):

1. **Post-quantum *authentication*, not only post-quantum secrecy.** Signal's
   PQ upgrade protects key agreement; identity keys and signatures in the
   systems above are classical. Maya Chat signs identities and handshakes with
   ML-DSA-65 (ADR-031).
2. **The chat identity is a chain account.** The same key, with the chain's
   own address derivation, so rotation and revocation can be anchored on a
   public ledger rather than trusted to a server's key directory.
3. **No phone number, no token needed to send.** SimpleX already has the
   first; Session and Utopia require neither a number nor (for sending) a
   payment, so this alone is not a difference either.

## Sources (retrieved 2026-09-28)

- SimpleX v5.6 quantum resistance: https://simplex.chat/blog/20240314-simplex-chat-v5-6-quantum-resistance-signal-double-ratchet-algorithm.html
- SimpleX PQ double ratchet spec: https://github.com/simplex-chat/simplexmq/blob/stable/protocol/pqdr.md
- Session Protocol V2: https://getsession.org/blog/session-protocol-v2
- Privacy Guides on Session V2: https://www.privacyguides.org/news/2025/12/03/session-messenger-adds-pfs-pqe-and-other-improvements/
- Signal SPQR: https://signal.org/blog/spqr/
- iMessage PQ3: https://security.apple.com/blog/imessage-pq3/
- Matrix PQ wrapper talk: https://cfp.2026.matrix.org/matrix-conference-2026/talk/TZ8ZYX/
- Briar maintenance mode: https://briarproject.org/news/2026-maintenance-mode/
- Utopia messenger: https://utopia-ecosystem.io/messenger/index.html
