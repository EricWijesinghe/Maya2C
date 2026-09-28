# ADR-031: Maya Chat — a detached, post-quantum, peer-to-peer messenger

**Status:** Accepted (first version); groups and metadata privacy are later
decisions named below.
**Date:** 2026-09-28

## Context

The owner's plan puts chat first among the applications built on Maya2C
(chat → wallet → AI chat → other services), each one a separate
application sharing the chain and nothing else it does not need. Chat is
the entry point because communication comes before finance.

Three requirements come from the vision and from this repository's own
rules:

1. **Post-quantum by default.** Messages recorded today must not be readable
   when large quantum computers arrive. The chain already signs with
   ML-DSA-65 and SLH-DSA and wraps its transport in ML-KEM-768.
2. **No company in the middle.** Identity must not be a phone number or an
   account on someone's server.
3. **Detached.** Chat must not link the node, hold funds, or need a token to
   send a message.

## Decision

A new application, `apps/chat` (crate `maya-chat`, binary `maya-chat`),
whose dependencies are `maya-crypto-pq`, `libp2p` and small pure-Rust crates.
It does not depend on `custom-l1-node`.

### Identity

A chat identity is an ML-DSA-65 key from the chain's suite registry. Its
**address is the chain's own address derivation** for that key
(`suite_address`), so a chat identity and an on-chain account can be the
same key. The chain later anchors rotation and revocation without a new
kind of name.

Each identity publishes a **prekey bundle**: an X-Wing encapsulation key,
signed by the identity key, with an expiry. X-Wing (ML-KEM-768 + X25519,
draft-connolly-cfrg-xwing-kem) is already in `crypto-pq` with the draft's
test vectors. Its combiner is secure if either half is, which protects
against both a quantum attacker and a bug in young ML-KEM code.

### One-to-one sessions

- **Handshake.** The initiator verifies the recipient's signed prekey,
  encapsulates to it, and signs the handshake with its own identity key. So
  both sides are authenticated and the secret is post-quantum.
- **Keys.** A session root is derived from the shared secret and the
  handshake transcript (BLAKE3 in key-derivation mode). One sending chain
  per direction steps forward with every message, and old message keys are
  deleted, giving forward secrecy within a session.
- **Messages.** ChaCha20-Poly1305 with the header as associated data. A
  bounded window of skipped keys admits out-of-order delivery; a replayed
  message is refused.
- **Re-keying.** A new X-Wing exchange, on demand or every N messages,
  provides post-compromise recovery. A full per-message KEM ratchet (like
  Signal's) is a later decision; its cost is ML-KEM's kilobyte-sized
  ciphertexts on every message.

### Delivery

- **Relays** are store-and-forward mailboxes. They hold encrypted envelopes
  for a recipient address until the recipient fetches them with a signed
  challenge. A relay sees ciphertext, sizes, timing and the recipient
  address, and nothing else.
- **Quotas and postage** bound abuse without asking anyone for tokens.
  Quotas apply per mailbox and across the relay. They cannot apply per
  sender, because a relay does not learn the sender (that stays inside the
  encryption), and addresses cost nothing to make. So every deposit
  carries a **postage stamp**: a proof of work over the envelope id, 20
  leading zero bits by default, which a relay may raise and which senders
  query with `Postage`. One message costs a fraction of a second. Filling
  one 1,000-envelope mailbox costs minutes of CPU, which raises the cost of
  a flood but does not remove it. Stronger answers need an identity
  signal: a proof-of-personhood stamp (`crates/personhood`), or
  contacts-only mailboxes. Each is its own later decision. *(Revised
  2026-09-28 after the security review: the first draft promised
  per-sender quotas, which this design cannot enforce.)*
- **Forged messages are cheap to refuse.** A forged `Chat` message can make
  a receiver derive up to `MAX_SKIP` (1,000) chain keys before its tag
  fails. That costs microseconds, and delivering the forgery through a
  relay costs the forger a postage stamp.
- **Transport** is libp2p (TCP, Noise, Yamux) with a request-response
  protocol, `/maya-chat/1`. Any peer can run a relay, and direct
  peer-to-peer delivery uses the same protocol.
- **Delay tolerance.** Envelopes carry their own expiry and are idempotent
  by id, so resending over a slow or intermittent link is safe. That is the
  property a Bundle Protocol (DTN) layer would build on later.

### What this first version does not do, and why

- **Group chat.** Groups will use MLS (RFC 9420) with a hybrid
  post-quantum ciphersuite, following the IETF drafts rather than a bespoke
  protocol. That needs an MLS library with such a ciphersuite, which is its
  own decision (ADR to follow).
- **Metadata privacy.** Relays learn who receives how much and when.
  Mixnet routing (Sphinx) is a later layer; the threat model says so plainly.
- **Chain anchoring of identities.** Addresses are already chain addresses;
  publishing prekeys and revocations on chain comes with the testnet.
- **Audit.** Like every cryptographic component here, it needs an external
  review before anyone relies on it for sensitive conversations. Until then
  it is marked RESEARCH.

## Consequences

- Chat runs on its own, with a relay on the owner's own machine at no cost,
  so it can be demonstrated before any testnet or funding.
- A signed X-Wing prekey is about 1.2 KB and an ML-DSA-65 signature about
  3.3 KB. Handshakes are therefore kilobytes, while ordinary messages
  are not (symmetric keys only).
- One identity scheme serves chat, the wallet and later apps, without any
  of them depending on the others.
