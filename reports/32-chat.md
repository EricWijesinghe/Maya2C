# 32 — Maya Chat MVP (Utopia Phase 2, first step)

Branch `feat/utopia-chat`, 2026-09-28. Design: [ADR-031](../docs/adr/ADR-031-maya-chat.md).
Status: **RESEARCH / working**. Unaudited: do not use it for sensitive conversations yet.

## What exists

`apps/chat` (crate `maya-chat`, library + `maya-chat` binary). It does not link the node.

| Piece | File | What it does |
|---|---|---|
| Identity | `src/identity.rs` | ML-DSA-65 key; its address is the chain's `suite_address` for that key, so one key can be both a chat identity and an account. Prekeys are X-Wing (ML-KEM-768 + X25519), derived from the seed per epoch and published in signed bundles |
| Session | `src/session.rs` | Signed handshake + X-Wing encapsulation → one chain per direction; each message key is used once and then dropped. Out-of-order delivery is handled with a window of up to 1,000 skipped keys, bounded in total. A failed decrypt leaves the state unchanged |
| Relay | `src/relay.rs` | Prekey directory and store-and-forward mailboxes. Per mailbox: 1,000 envelopes or 16 MiB. Across the relay: 1 GiB, 100k bundles, 10k challenges. Envelopes are 256 KiB max and expire within 30 days. Only the owner can empty a mailbox, by signing a challenge that is valid for 120 s |
| Client | `src/client.rs` | Sessions keyed by peer. A new handshake re-keys the session; a handshake it has already accepted is refused as a replay |
| Network | `src/net.rs` | `/maya-chat/1`: libp2p request-response, CBOR bodies, TCP + Noise + Yamux, size caps |
| CLI | `src/main.rs` | `init`, `address`, `relay`, `publish`, `send`, `recv` |

Two defects were found by reading the code and fixed before any tests were written:

- **Replayed handshakes.** A replayed `Open` envelope reset a live session to stale keys and delivered its first message again. The client now remembers accepted handshake ids for the 7-day handshake window.
- **Unbounded relay memory.** The dedup set, challenges, bundles and the mailbox count could grow without limit, because addresses cost nothing to make. Each is now pruned by expiry and capped.

### Security review (security-reviewer agent — not an audit)

| Finding | Severity | Resolution |
|---|---|---|
| Any peer could fill a victim's mailbox; the ADR promised per-sender quotas, which cannot exist because relays do not learn senders | HIGH | Proof-of-work postage on every deposit (20 bits default, relay-adjustable, `Request::Postage`); stamp-independent dedup id; ADR-031 revised. **Partly mitigated**: a flood now costs minutes of CPU per mailbox, not nothing |
| CLI rebuilt the client on every run: follow-up messages were unreadable and replayed handshakes were accepted again | HIGH | `Client::save`/`restore`: `DIR/sessions.bin`, ChaCha20-Poly1305 under a seed-derived key, written atomically, saved only after the relay accepts. The e2e test now exchanges three messages across separate processes |
| A forged message makes the receiver derive up to 1,000 keys before the tag fails | MEDIUM | Accepted and documented: microseconds of work, and reaching a mailbox costs the forger a stamp |
| `session::initiate` trusted its caller to check the bundle's owner | LOW | `initiate(me, to, bundle, now)` refuses a bundle that is not `to`'s |

## Measured

Run on the owner's Windows 11 workstation (4 build jobs), commit following 8e0d108.

- `cargo test -p maya-chat`: 28 passed, 0 failed.
  - `session_tests`: 17.
  - `relay_tests`: 9.
  - `net_e2e_tests`: 2, 0.93 s. This is the real binary: a relay on 127.0.0.1 at the default 20-bit postage, two identities, and four stamped sends across separate processes (including a reply and a follow-up that only work because sessions persist).
- `cargo test -p maya-crypto-pq`: 89 passed, 0 failed.
- `cargo nextest run -p custom-l1-node --test suite_parity_tests --test ledger_fixture_tests`: 13 passed. This confirms addresses are unchanged after `suite_address` moved into `crypto-pq`.
- `cargo clippy -p maya-chat --all-targets`: no warnings in `maya-chat`. One warning remains, `doc_markdown` in `crates/crypto-pq/src/kem.rs:31`, and it was already on HEAD.
- `scripts/lint_debt.sh --check`: 1140 (baseline 1140).
- `cargo deny check`: advisories, bans, licenses and sources all ok.
- `cargo xtask coverage --verify-targets`: all claims backed. `cargo xtask claims-check`: 0 unauthorised claims.

No performance figures were measured.

## Not yet — and what gets us there

| Gap | Next step | Cost | Who |
|---|---|---|---|
| External security review | Apply to NLnet NGI Zero (draft in `docs/funding/`); NLnet-funded projects can get a free Radically Open Security audit | Free; the application takes 1–2 h | Owner submits |
| No interactive mode (one command per message) | A `chat` REPL over the saved session store | Code only | Claude |
| Seed stored unencrypted in `identity.key` | Passphrase keystore shared with `maya-wallet-core` | Code only | Claude |
| Group chat | MLS (RFC 9420) via `openmls`, with the PQ ciphersuite decision written as an ADR | Code only | Claude, then review |
| Post-compromise security | Per-message KEM ratchet (as in SPQR / PQ3) | Code only | Claude, then review |
| Metadata privacy (relays see who receives, when and how much) | Mixnet (e.g. Nym/Katzenpost) | Research first | Later phase |
| Public relay | Deploy one relay on a free-tier VM | Free tier | Owner, with `APPROVED: chat-relay-deploy` |
| Flooding a mailbox still costs only minutes of CPU | Proof-of-personhood stamp (`crates/personhood`) or contacts-only mailboxes | Code only, needs an ADR | Claude |
